//! Definition-local expression checking. No callee implementation is consulted.
use hirsel_type_analysis::cleanup::{
    CleanupLeaf, cleanup_summary, declared_cleanup, parameter_cleanup,
};
mod calls;
mod comptime;
mod contracts;
mod formats;
mod inject;
mod intrinsics;
mod iteration;
mod literals;
mod patterns;
mod shapes;
mod source_rows;
mod tails;
#[cfg(test)]
mod tests;
mod type_display;
mod types;
use hirsel_check_protocol::body::*;
use hirsel_diag::{Diagnostic, RuleId};
use hirsel_intern::Symbol;
use hirsel_intern::work::{self, Counter};
use hirsel_span::Span;
use hirsel_syntax as ast;
use hirsel_syntax::DefId;
use hirsel_types::*;
use std::collections::{BTreeMap, BTreeSet};

pub struct Input<'a> {
    /// The session interner: every row read or produced lives here.
    pub types: &'a TypeInterner,
    /// Implementation-side structural memos (substitution).
    pub memo: &'a TypeMemo,
    pub syntax: &'a ast::ItemSyntax,
    pub context: &'a ast::FileContext,
    pub resolution: &'a Resolution,
    pub signature: &'a Signature,
}
/// Registry-supplied operation identities; no driver dependency or source lookup.
pub type LoopProtocolPaths = (&'static str, ast::DefPath, ast::DefPath);
/// Additive input for callers that provide executable cursor protocols.
pub struct LoopInput<'a> {
    pub body: Input<'a>,
    pub protocols: &'a [LoopProtocolPaths],
}
pub struct Output {
    pub checked: QueryOutput<CheckedBody>,
    pub plans: Plans,
}
/// Recovery is absorbing; language diagnostics accompany complete recovery tables.
/// Provider operational/unavailable errors propagate without a fabricated RuleId.
/// Check using provider-backed cursor declarations selected by plain registry data.
pub fn check_with_loop_protocols(
    input: LoopInput<'_>,
    provider: &mut dyn SignatureProvider,
) -> Result<Output, QueryError> {
    let failure = std::cell::RefCell::new(None);
    let result = check_inner(input, provider, &failure);
    failure.into_inner().map_or(result, Err)
}
fn check_inner(
    input: LoopInput<'_>,
    provider: &mut dyn SignatureProvider,
    failure: &std::cell::RefCell<Option<QueryError>>,
) -> Result<Output, QueryError> {
    let LoopInput {
        body: input,
        protocols,
    } = input;
    let format_holes = input
        .syntax
        .tree
        .formats
        .iter()
        .filter_map(|format| {
            match &input.syntax.tree.expressions[format.expression.0 as usize].kind {
                ast::ExprKind::Block {
                    tail: Some(tail), ..
                } => Some((*tail, format.holes.clone())),
                _ => None,
            }
        })
        .collect();
    let mut c = Checker {
        body: CheckedBody {
            lexical: input.syntax.owner(),
            tails: vec![],
            places: vec![],
            calls: vec![],
            resolution: input.resolution.clone(),
            bindings: vec![],
            patterns: vec![],
            expressions: vec![],
            instantiations: vec![],
            closures: vec![],
        },
        source_rows: source_rows::SourceRows::new(input.resolution),
        lexical: input.syntax.owner(),
        types: input.types,
        memo: input.memo,
        input,
        provider,
        loop_protocols: protocols,
        source_places: BTreeMap::new(),
        frames: vec![],
        work: vec![],
        function_value_arguments: BTreeMap::new(),
        values: BTreeMap::new(),
        diagnostics: vec![],
        dependencies: BTreeMap::new(),
        snapshots: BTreeMap::new(),
        representations: BTreeMap::new(),
        property_normalizations: Default::default(),
        plans: Plans::default(),
        calls: vec![],
        format_holes,
        formatters: BTreeMap::new(),
        ranges: BTreeMap::new(),
        range_sources: BTreeSet::new(),
        expected_sites: BTreeMap::new(),
        inference_variables: BTreeSet::new(),
        result_tail_mismatches: BTreeMap::new(),
        mut_aliases: BTreeSet::new(),
        proof_failure: failure,
    };
    let item = &c.input.syntax.item;
    let owner = NameSite::Item;
    match (&item.kind, &c.input.signature.kind) {
        (
            ast::ItemKind::Function {
                signature, body, ..
            },
            SignatureKind::Function(f),
        ) => {
            let f = c.import_function(f)?;
            let frame = c.frame(owner, None, &f, signature);
            if let Some(body) = body
                && c.diagnostics.is_empty()
            {
                c.final_return(*body);
                if hirsel_check_protocol::fact::fact_owner(c.input.signature.definition.path())
                    .is_some()
                {
                    c.expected_sites
                        .insert(*body, (c.span(*body), const { RuleId::of("FACT-2") }));
                }
                c.work
                    .push(Work::Enter(*body, c.frames[frame].result, frame));
            }
        }
        (
            ast::ItemKind::Implementation { signature, .. },
            SignatureKind::Implementation(actuals),
        ) => {
            let family = actuals.family.path().clone();
            // Record every declaration the matcher reads as a dependency.
            if let SignatureKind::Family(declared) = &c.snapshot(&family)?.kind {
                for member in &declared.members {
                    if matches!(member.kind, FamilyMemberKind::Operation) {
                        c.snapshot(&family_operation_path(&family, &member.name))?;
                    }
                }
            }
            for (_, actual) in &actuals.members {
                if let MemberActual::Operation(target) = actual {
                    c.snapshot(target)?;
                }
            }
            let types = c.types;
            let memo = c.memo;
            let refusal = crate::implementation::match_implementation(
                types,
                memo,
                &family,
                actuals,
                &mut *c.provider,
            )?;
            if let Err(refusal) = refusal {
                let span = match item.kind {
                    ast::ItemKind::Implementation { ref members, .. } => members
                        .iter()
                        .find(|m| m.name().as_str() == refusal.member)
                        .map_or(item.name_span, |m| m.origin().span),
                    _ => item.name_span,
                };
                let _ = signature;
                c.report(
                    refusal.rule,
                    span,
                    &format!(
                        "implementation does not match `{family}` at member `{}`: {}",
                        refusal.member, refusal.message
                    ),
                );
            }
        }
        (ast::ItemKind::Const { value, .. }, SignatureKind::Const { ty: ty, .. }) => {
            let ty = c.import(*ty)?;
            let frame = c.empty_frame(owner, None, Some(ty), None, false);
            c.work.push(Work::Enter(*value, Some(ty), frame));
        }
        (
            ast::ItemKind::Type {
                drop: Some(drop), ..
            },
            SignatureKind::Type(declaration),
        ) => {
            let generics = declaration.generics.clone();
            let unit = c.ty(TypeKind::Tuple(vec![].into()));
            let frame = c.empty_frame(owner, None, Some(unit), None, false);
            let scope = c.site(owner);
            c.body
                .resolution
                .generic_declarations
                .insert(scope, generics.clone());
            let arguments = generics
                .iter()
                .enumerate()
                .map(|(i, generic)| match generic {
                    GenericParameter::Type { .. } => GenericArgument::Type(c.ty(
                        TypeKind::Parameter(TypeParameterRef::body(scope, TypeParamId(i as u32))),
                    )),
                    GenericParameter::Size { .. } => GenericArgument::Size(Size::Parameter(
                        SizeParameterRef::body(scope, SizeParamId(i as u32)),
                    )),
                })
                .collect();
            let binding = c.input.resolution.drop_binding.ok_or_else(|| {
                QueryError::Operational(OperationalFailure {
                    operation: "finalizer binding".into(),
                    message: "resolver omitted the finalizer binder identity".into(),
                })
            })?;
            let own = c.ty(TypeKind::Named {
                definition: DefId::intern(&item.definition),
                arguments,
            });
            c.body.bindings.push(BindingDecision {
                owner: NameSite::Item,
                binding: ResolvedName::Local(binding),
                site: NameSite::Item,
                ty: own,
                usage: UseKind::Borrow,
                mutable: false,
            });
            c.frames[frame].locals.insert(binding, own);
            c.frames[frame].finalizer = true;
            c.expected_sites.insert(
                drop.body,
                (c.span(drop.body), const { RuleId::of("DROP-3") }),
            );
            c.work.push(Work::Enter(drop.body, Some(unit), frame));
        }
        _ => (),
    }
    while let Some(work) = c.work.pop() {
        c.step(work)?;
    }
    c.check_contracts()?;
    c.check_closed_comptimes()?;
    c.check_compile_time_values();
    c.publish_tails()?;
    for f in &c.frames {
        if let Some(result) = f.result {
            c.plans.functions.push(FunctionPlan {
                owner: f.owner.clone(),
                result,
            });
        }
    }
    c.body.expressions.sort_by_key(|e| e.expression);
    c.body.patterns.sort_by_key(|p| p.pattern);
    c.body
        .bindings
        .sort_by_key(|b| (b.owner.clone(), b.site.clone(), binding_key(&b.binding)));
    c.body.instantiations.sort_by_key(|e| e.expression);
    c.body.closures.sort_by_key(|e| e.expression);
    c.body.calls.sort_by_key(|p| p.expression);
    c.body.places = c.source_places.into_values().collect();
    c.plans.tries.sort_by_key(|p| p.expression);
    c.plans.loops.sort_by_key(ForPlan::statement);
    c.diagnostics.sort_by_key(|d| {
        (
            d.primary.start,
            d.primary.end,
            d.rule.prefix().to_owned(),
            d.rule
                .as_str()
                .split_once('-')
                .unwrap()
                .1
                .parse::<u32>()
                .unwrap(),
        )
    });
    c.diagnostics.dedup();
    Ok(Output {
        checked: QueryOutput {
            value: c.body,
            diagnostics: c.diagnostics,
            dependencies: c
                .dependencies
                .into_iter()
                .map(|(definition, fingerprint)| SignatureDependency {
                    definition,
                    fingerprint,
                })
                .collect(),
        },
        plans: c.plans,
    })
}
/// A definition named by its path or its session id.
pub(super) trait AsDef {
    fn def(&self) -> DefId;
}
impl AsDef for DefId {
    fn def(&self) -> DefId {
        *self
    }
}
impl AsDef for ast::DefPath {
    fn def(&self) -> DefId {
        DefId::intern(self)
    }
}
impl<T: AsDef + ?Sized> AsDef for &T {
    fn def(&self) -> DefId {
        (**self).def()
    }
}
fn binding_key(name: &ResolvedName) -> (u8, u32) {
    match name {
        ResolvedName::Local(l) => (0, l.0),
        ResolvedName::Parameter(p) => (1, p.0),
        _ => (2, 0),
    }
}
struct Frame {
    owner: NameSite,
    parent: Option<usize>,
    result: Option<TypeId>,
    error: Option<TypeId>,
    incomplete: bool,
    finalizer: bool,
    parameters: Vec<TypeId>,
    locals: BTreeMap<LocalId, TypeId>,
    loops: Vec<ast::StmtId>,
    /// Enclosing labelled blocks, innermost last (LANG-11).
    blocks: Vec<LabelledBlock>,
    restricted: Vec<(ResolvedName, Span)>,
}
/// A labelled block being checked: its result type is its expected type, or
/// the first typed break value or tail (LANG-11).
struct LabelledBlock {
    label: Symbol,
    /// How many loops enclosed the block, so an unlabelled exit inside it
    /// can tell whether its innermost loop lies outside the block.
    loops: usize,
    ty: Option<TypeId>,
    /// Whether the tail was checked against `ty`, which then reported any mismatch.
    tail_checked: bool,
}
/// Each work item owns its lexical frame and expected type; exits never change
/// a global current-function pointer. Both static branches always get checked.
enum Work {
    Enter(ast::ExprId, Option<TypeId>, usize),
    Finish(ast::ExprId, Option<TypeId>, usize),
    Statement(ast::StmtId, usize),
    AfterChild(After),
}
enum After {
    SequenceFirst(ast::ExprId, usize),
    Binary(ast::ExprId, Option<TypeId>, usize),
    CallCallee(ast::ExprId, Option<TypeId>, usize),
    CallArgument(usize, usize),
    FormatAppend(ast::ExprId, Option<TypeId>, usize),
    Let(ast::StmtId, usize),
    Assign(ast::StmtId, usize),
    AssignPlace(ast::StmtId, usize),
    Match(ast::ExprId, Option<TypeId>, usize),
    MatchArm(ast::ExprId, usize, Option<TypeId>, usize, bool),
    IfThen(ast::ExprId, Option<TypeId>, usize),
    For(ast::StmtId, usize),
    LabelledTail(ast::ExprId, Option<TypeId>, usize),
    Closure(ast::ExprId, Option<TypeId>, usize, usize, FunctionType),
    LocalFunction(ast::StmtId, usize, usize, FunctionType),
    LoopEnd(usize, usize),
}
/// Completed property normalizations under the checker's current input view.
/// As in rustc traits/project.rs:310-319,350-365, a type alone is not the key:
/// generic declarations and the available nominal signatures must agree.
#[derive(Default)]
struct PropertyNormalizations {
    context: Option<(GenericDeclarations, usize)>,
    rows: std::collections::HashMap<TypeId, TypeId, hirsel_intern::IdHasher>,
}
impl PropertyNormalizations {
    fn bind(&mut self, generics: &GenericDeclarations, signatures: usize) {
        if !self
            .context
            .as_ref()
            .is_some_and(|(known, count)| *count == signatures && known == generics)
        {
            self.rows.clear();
            self.context = Some((generics.clone(), signatures));
        }
    }
}
struct Checker<'a, 'p> {
    input: Input<'a>,
    types: &'a TypeInterner,
    memo: &'a TypeMemo,
    /// The owner of this item's lexical sites, which rows qualify them with.
    lexical: LexicalOwner,
    provider: &'p mut dyn SignatureProvider,
    loop_protocols: &'a [LoopProtocolPaths],
    body: CheckedBody,
    source_rows: source_rows::SourceRows,
    source_places: BTreeMap<ast::ExprId, SourcePlace>,
    frames: Vec<Frame>,
    work: Vec<Work>,
    function_value_arguments: BTreeMap<ast::ExprId, Vec<GenericArgument>>,
    values: BTreeMap<ast::ExprId, TypeId>,
    diagnostics: Vec<Diagnostic>,
    dependencies: BTreeMap<DefId, ast::ContentHash>,
    snapshots: BTreeMap<DefId, SignatureRef>,
    /// Each instantiated representation imported so far. The store is
    /// append-only, so a nominal type is expanded into it once rather than
    /// once per projection of its fields (GAP 5).
    representations: BTreeMap<ast::DefPath, Vec<(Vec<GenericArgument>, TypeId)>>,
    property_normalizations: PropertyNormalizations,
    plans: Plans,
    calls: Vec<calls::PendingCall>,
    /// F-string callees (SEM-3) with their selected operation and the declared
    /// ordinals of their written arguments.
    formatters: BTreeMap<ast::ExprId, (ast::DefPath, Vec<usize>)>,
    /// Compiler-created f-string finishes indexed once per body.
    format_holes: BTreeMap<ast::ExprId, Vec<ast::ExprId>>,
    ranges: BTreeMap<ast::ExprId, TypeId>,
    range_sources: BTreeSet<ast::ExprId>,
    expected_sites: BTreeMap<ast::ExprId, (Span, RuleId)>,
    inference_variables: BTreeSet<TypeParameterRef>,
    /// Keep the original LANG-7 while validating propagation before offering a repair.
    result_tail_mismatches: BTreeMap<ast::ExprId, (TypeId, usize)>,
    mut_aliases: BTreeSet<LocalId>,
    proof_failure: &'p std::cell::RefCell<Option<QueryError>>,
}
impl Checker<'_, '_> {
    /// A site of this item, as rows name it.
    fn site(&self, site: NameSite) -> SiteRef {
        self.lexical.site(site)
    }
    /// Whether a relation place is a frame's own Arena parameter (PROV-4).
    fn arena_place(&self, o: &Origin) -> bool {
        let Some(heap) = &self.input.signature.metadata.roles.heap else {
            return false;
        };
        let OriginRoot::Parameter(ParameterRef::Body { scope, parameter }) = &o.root else {
            return false;
        };
        o.fields.is_empty()
            && self
                .frames
                .iter()
                .find(|f| self.site(f.owner) == *scope)
                .and_then(|f| f.parameters.get(parameter.0 as usize))
                .is_some_and(|t| hirsel_types::is_arena_type(self.types, *t, &heap.arena))
    }
    /// The arena places a row's relation names.
    fn arena_places(&self, ty: TypeId) -> Vec<Origin> {
        self.types.get(ty).map_or_else(Vec::new, |row| {
            row.modes
                .relation
                .reaches()
                .iter()
                .filter_map(|r| match r {
                    Reach::Place(o) if self.arena_place(o) => Some(o.clone()),
                    _ => None,
                })
                .collect()
        })
    }
    /// A relation's arena places: where its blocks live (PROV-4).
    fn placement(&self, relation: &Relation) -> Relation {
        relation.retain(|r| matches!(r, Reach::Place(o) if self.arena_place(o)))
    }
    fn proof_failure<T>(&self, error: QueryError, fallback: T) -> T {
        self.proof_failure.borrow_mut().get_or_insert(error);
        fallback
    }
    fn span(&self, id: ast::ExprId) -> Span {
        self.input.syntax.tree.expressions[id.0 as usize]
            .origin
            .span
    }
    fn report(&mut self, rule: RuleId, span: Span, message: &str) {
        self.diagnostics
            .push(Diagnostic::error(rule, span, message));
    }
    fn expression_type_argument_diagnostic(&mut self, id: ast::ExprId) {
        // Keep the prohibited index's range and recovery. Unlike Rust's
        // generic_args_prohibited handler, deletion is not a general repair:
        // the call may need context that only the written type supplied.
        let mut diagnostic = Diagnostic::error(
            const { RuleId::of("LANG-2") },
            self.span(id),
            "type arguments are never written in expressions",
        );
        diagnostic.notes.push("help: postfix `[...]` indexes values. Generic arguments come from declared signatures, already typed arguments, or expected types; write type arguments in type annotations".into());
        diagnostic.notes.push("help: for `type Holder[T] { item:T }`, write `let value:Holder[u64]=Holder(item:1u64)`. For `fn id[T](take x:T)->T {take x}`, write `id(1u64)` or `let value:u64=id(1)`".into());
        diagnostic.notes.push("help: when the result type does not determine a generic parameter, type an argument first, for example `let argument:u64=1; consume(argument)` for `fn consume[T](x:T) {}`. Later arguments never supply a type for an earlier literal".into());
        self.diagnostics.push(diagnostic);
    }
    fn error(&mut self) -> TypeId {
        self.ty(TypeKind::Error)
    }
    fn ty(&mut self, kind: TypeKind) -> TypeId {
        // Unit is plain and owns no cleanup, including implicit block results.
        // These facts agree with signature normalization and need no body proof.
        let unit = matches!(&kind, TypeKind::Tuple(ts) if ts.is_empty());
        let mut modes = Modes::global();
        if unit {
            modes.sendability = Sendability::Sendable;
        }
        let component = |id| {
            self.types
                .get(id)
                .map_or(DropAllocation::MayAllocate, |t| t.drop_allocation.clone())
        };
        let drop_allocation = cleanup_summary(&kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => component(t),
            // Body parameters retain conditional cleanup until the bound check.
            CleanupLeaf::Parameter(p) => {
                parameter_cleanup(&self.body.resolution.generic_declarations, p)
            }
            CleanupLeaf::Nominal(definition, arguments) => {
                let signature = if definition == self.input.signature.definition {
                    Some(self.input.signature)
                } else {
                    self.snapshots.get(&definition).map(|s| s.as_ref())
                };
                match signature {
                    Some(Signature {
                        kind: SignatureKind::Type(d),
                        ..
                    }) => declared_cleanup(d, self.types, arguments, component),
                    _ => DropAllocation::MayAllocate,
                }
            }
        });
        let id = self.types.intern(Type {
            modes,
            drop_allocation,
            ..Type::with_kind(kind)
        });
        self.normalize_properties(id)
    }
    fn inferred_aggregate_type(&mut self, kind: TypeKind) -> TypeId {
        let root = self.ty(kind.clone());
        let mut row = self.types.get(root).unwrap().clone();
        row.kind = kind;
        self.types.intern(row)
    }
    fn normalize_properties(&mut self, id: TypeId) -> TypeId {
        // Snapshots are inserted once and never replaced during a body query.
        // Their count is therefore a view revision; own signature is immutable.
        // Generic declarations can change without the signature view growing.
        self.property_normalizations.bind(
            &self.body.resolution.generic_declarations,
            self.snapshots.len(),
        );
        if let Some(normalized) = self.property_normalizations.rows.get(&id).copied() {
            #[cfg(test)]
            assert_eq!(
                normalized,
                self.normalize_properties_uncached(id).unwrap(),
                "cached body normalization differs from the original traversal"
            );
            return normalized;
        }
        match self.normalize_properties_uncached(id) {
            Ok(normalized) => {
                self.property_normalizations.rows.insert(id, normalized);
                normalized
            }
            Err(error) => self.proof_failure(error, id),
        }
    }
    fn normalize_properties_uncached(&self, id: TypeId) -> Result<TypeId, QueryError> {
        let generics = &self.body.resolution.generic_declarations;
        let prior = self.types.get(id).unwrap().modes.sendability.clone();
        let normalized = normalize_type_properties(self.types, id, generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(if path == self.input.signature.definition {
                Some(std::sync::Arc::new(self.input.signature.clone()))
            } else {
                self.snapshots.get(&path).cloned()
            })
        })?;
        Ok(
            if prior == Sendability::Sendable
                && self.types.get(id).unwrap().sendability_requirement
                    == SendabilityRequirement::Sendable
            {
                let mut row = self.types.get(normalized).unwrap().clone();
                row.modes.sendability = prior;
                self.types.intern(row)
            } else {
                normalized
            },
        )
    }
    /// The arena an expected row places constructions in (PROV-4). Call-result
    /// hints still use the callee's allocator namespace here, whose bound
    /// references are not this body's arenas; J instantiates those
    /// declarations from actual caller operands.
    fn construction_placement(&self, expected: TypeId) -> Option<Relation> {
        let placement = self.placement(&self.types.get(expected).unwrap().modes.relation);
        (!placement.is_global()).then_some(placement)
    }
    fn construction_type(&mut self, actual: TypeId, expected: TypeId) -> TypeId {
        let Some(placement) = self.construction_placement(expected) else {
            return actual;
        };
        let mut row = self.types.get(actual).unwrap().clone();
        row.modes.relation = row.modes.relation.union(&placement);
        self.types.intern(row)
    }
    // A comparison operand only borrows, so the type it inherits from the
    // other side drops its `unique` qualifiers (MODE-2).
    fn compared_operand_type(&mut self, op: ast::BinaryOp, t: TypeId) -> TypeId {
        if matches!(
            op,
            ast::BinaryOp::Eq
                | ast::BinaryOp::Ne
                | ast::BinaryOp::Lt
                | ast::BinaryOp::Le
                | ast::BinaryOp::Gt
                | ast::BinaryOp::Ge
        ) {
            hirsel_types::erase_uniqueness(self.types, t).unwrap_or(t)
        } else {
            t
        }
    }
    fn placed_part_type(&mut self, owner: TypeId, part: TypeId) -> TypeId {
        if self.construction_placement(owner).is_none() {
            return part;
        }
        let generics = self.body.resolution.generic_declarations.clone();
        placed_part_type(self.types, owner, part, &generics, &mut |path| {
            Ok::<_, hirsel_types::QueryError>(if path == self.input.signature.definition {
                Some(std::sync::Arc::new(self.input.signature.clone()))
            } else {
                self.snapshots.get(&path).cloned()
            })
        })
        .unwrap_or_else(|error| self.proof_failure(error, part))
    }
    fn projected_type(&mut self, owner: TypeId, part: TypeId) -> TypeId {
        let part = self.placed_part_type(owner, part);
        let generics = self.body.resolution.generic_declarations.clone();
        let projected = {
            let mut lookup = |path: DefId| {
                Ok::<_, hirsel_types::QueryError>(if path == self.input.signature.definition {
                    Some(std::sync::Arc::new(self.input.signature.clone()))
                } else {
                    self.snapshots.get(&path).cloned()
                })
            };
            let mut memo = ContextMemo::new(self.types, &generics, &mut lookup);
            memo.projected(self.memo, owner, part)
        };
        projected.unwrap_or_else(|error| self.proof_failure(error, part))
    }
    // MEM-7 / #98: inspection projects payloads; whole bindings retain the box.
    fn box_payload(&mut self, mut ty: TypeId) -> TypeId {
        while let TypeKind::Box(part) = self.kind(ty) {
            ty = self.projected_type(ty, part);
        }
        ty
    }
    fn project_box_place(&mut self, id: ast::ExprId, base: ast::ExprId) {
        let mut ty = self.value(base);
        let mut place = self
            .source_place(base)
            .cloned()
            .expect("checked box receiver");
        place.expression = id;
        while let TypeKind::Box(part) = self.kind(ty) {
            place.projections.push(SourceProjection {
                expression: id,
                kind: SourceProjectionKind::Field(FieldId(0)),
            });
            ty = part;
        }
        self.record_place(place);
    }
    fn primitive(&mut self, p: Primitive) -> TypeId {
        let id = self.ty(TypeKind::Primitive(p));
        let mut row = self.types.get(id).unwrap().clone();
        row.drop_allocation = DropAllocation::NoAlloc;
        self.types.intern(row)
    }
    /// RUN-3's diagnostic where an entry is expected and none forms.
    const ENTRY_FORMATION: &str = "an entry names a top-level, non-generic, non-extern, complete function definition of the entry's type";
    fn kind(&self, ty: TypeId) -> TypeKind {
        self.types.get(ty).unwrap().kind.clone()
    }
    fn invalid(&self, ty: TypeId) -> bool {
        matches!(self.kind(ty), TypeKind::Error)
    }
    fn bottom(&self, ty: TypeId) -> bool {
        matches!(self.kind(ty), TypeKind::Primitive(Primitive::Never))
    }
    fn value(&self, id: ast::ExprId) -> TypeId {
        self.values[&id]
    }
    fn place_root(&self, id: ast::ExprId) -> SourceRoot {
        let name = self.body_name(id).map(|(_, n)| n);
        match name {
            // A bare use of a pattern's provisional local that its checked
            // pattern replaced by a nullary variant is that variant, a value
            // (`path_type`); no binding holds it.
            Some(ResolvedName::Local(local)) if self.superseded_pattern_local(*local) => {
                SourceRoot::Value(id)
            }
            Some(name @ (ResolvedName::Local(_) | ResolvedName::Parameter(_))) => {
                let scope = self.body_scope(NameSite::Expression(id)).map(|(_, s)| s);
                let binding = self.body.bindings.iter().find(|b| {
                    b.binding == *name
                        && (matches!(name, ResolvedName::Local(_))
                            || scope.is_none_or(|scope| scope == &b.owner))
                });
                let binding = binding.expect("checked scoped binding root");
                SourceRoot::Binding {
                    owner: binding.owner.clone(),
                    binding: binding.binding.clone(),
                }
            }
            Some(ResolvedName::Definition(d)) => {
                let mut path = d.clone();
                let mut seen = BTreeSet::new();
                while seen.insert(path.clone()) {
                    let Some(target) = self
                        .snapshots
                        .get(&DefId::intern(&path))
                        .and_then(|s| s.metadata.constant_alias.as_deref())
                    else {
                        break;
                    };
                    path = target.clone();
                }
                SourceRoot::Definition(path)
            }
            _ => SourceRoot::Value(id),
        }
    }
    /// A written callable type's guarantees, its state promises and its
    /// result's `@captures` selection, belong to the binding it annotates at
    /// any depth, whatever the initializer happens to promise; I/J hold the
    /// initializer to them (contract 6, CLO-4, MEM-14).
    fn written_promises(&mut self, actual: TypeId, written: TypeId) -> TypeId {
        if self.invalid(actual) || self.invalid(written) || !self.compatible(actual, written) {
            return actual;
        }
        let mut row = self.types.get(actual).unwrap().clone();
        let kind = match (row.kind.clone(), self.kind(written)) {
            (TypeKind::Function(mut held), TypeKind::Function(promised)) => {
                held.state_promises = promised.state_promises.clone();
                // A callable header is the binding's declared contract. The
                // initializer keeps its actual header for I/J's receipt check;
                // omitted modes in this header cannot inherit its promises.
                held.parameters = promised.parameters;
                held.result = promised.result;
                held.error = promised.error;
                TypeKind::Function(held)
            }
            (TypeKind::Tuple(parts), TypeKind::Tuple(promised)) => TypeKind::Tuple(
                parts
                    .iter()
                    .zip(promised.iter())
                    .map(|(part, promised)| self.written_promises(*part, *promised))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (TypeKind::Box(part), TypeKind::Box(promised)) => {
                TypeKind::Box(self.written_promises(part, promised))
            }
            (
                TypeKind::Builtin { kind, arguments },
                TypeKind::Builtin {
                    arguments: promised,
                    ..
                },
            ) => TypeKind::Builtin {
                kind,
                arguments: self.written_argument_promises(&arguments, &promised),
            },
            (
                TypeKind::Named {
                    definition,
                    arguments,
                },
                TypeKind::Named {
                    arguments: promised,
                    ..
                },
            ) => TypeKind::Named {
                definition,
                arguments: self.written_argument_promises(&arguments, &promised),
            },
            _ => return actual,
        };
        if kind == row.kind {
            return actual;
        }
        row.kind = kind;
        self.types.intern(row)
    }
    fn written_argument_promises(
        &mut self,
        arguments: &[GenericArgument],
        promised: &[GenericArgument],
    ) -> SharedSlice<GenericArgument> {
        arguments
            .iter()
            .zip(promised.iter())
            .map(|pair| match pair {
                (GenericArgument::Type(held), GenericArgument::Type(promised)) => {
                    GenericArgument::Type(self.written_promises(*held, *promised))
                }
                (argument, _) => argument.clone(),
            })
            .collect::<Vec<_>>()
            .into()
    }
    fn source_place(&self, expression: ast::ExprId) -> Option<&SourcePlace> {
        work::count(Counter::BodySourcePlaceRows);
        self.source_places.get(&expression)
    }
    fn record_place(&mut self, place: SourcePlace) {
        work::count(Counter::BodySourcePlaceRows);
        self.source_places.insert(place.expression, place);
    }
    fn project_place(&mut self, id: ast::ExprId, base: ast::ExprId, kind: SourceProjectionKind) {
        let mut place = self
            .source_place(base)
            .cloned()
            .expect("checked projection receiver");
        place.expression = id;
        place.projections.push(SourceProjection {
            expression: id,
            kind,
        });
        place.access = SourceAccess::Read;
        self.record_place(place);
    }
    fn finish_place(&mut self, id: ast::ExprId) {
        use ast::{ExprKind as E, UnaryOp as U};
        match self.input.syntax.tree.expressions[id.0 as usize].kind {
            E::Index { base, index } => {
                self.project_box_place(id, base);
                self.project_place(id, id, SourceProjectionKind::Index(index))
            }
            E::Unary {
                op: op @ (U::Copy | U::Take | U::Mut | U::Unique),
                value,
            } => {
                let mut place = self
                    .source_place(value)
                    .cloned()
                    .expect("checked access operand");
                place.expression = id;
                place.access = match op {
                    U::Copy => SourceAccess::Copy,
                    U::Take => SourceAccess::Take,
                    U::Mut => SourceAccess::Mut,
                    U::Unique => SourceAccess::Unique,
                    _ => unreachable!(),
                };
                self.record_place(place);
            }
            _ if self.source_place(id).is_none() => self.record_place(SourcePlace {
                expression: id,
                root: self.place_root(id),
                projections: vec![].into(),
                access: SourceAccess::Read,
            }),
            _ => (),
        }
    }
    fn put(
        &mut self,
        id: ast::ExprId,
        mut ty: TypeId,
        expected: Option<TypeId>,
        usage: UseKind,
    ) -> Result<(), QueryError> {
        if let Some(expected) = expected {
            match self.input.syntax.tree.expressions[id.0 as usize].kind {
                ast::ExprKind::Literal(_) => ty = self.placed_part_type(expected, ty),
                ast::ExprKind::Tuple(_) | ast::ExprKind::Sequence(_) => {
                    ty = self.construction_type(ty, expected);
                }
                _ => (),
            }
            // `?` owns the registered-Result prerequisite. Its success context
            // must not turn a wrong operand family into a secondary LANG-7.
            let wrong_try_family = self.partial_result(expected).is_some_and(|(definition, _)| {
                !matches!(self.kind(ty), TypeKind::Named { definition: actual, .. } if actual == definition)
            });
            if !self.invalid(ty)
                && !self.invalid(expected)
                && !self.bottom(ty)
                && !wrong_try_family
                && !self.compatible(ty, expected)
            {
                let (span, rule) = self
                    .expected_sites
                    .get(&id)
                    .cloned()
                    .unwrap_or((self.span(id), const { RuleId::of("LANG-7") }));
                // RUN-3: only naming a matching top-level definition forms an entry.
                let entry =
                    self.entry_signature(expected).is_some() && self.entry_signature(ty).is_none();
                let (span, rule, message) = if entry {
                    (
                        self.span(id),
                        const { RuleId::of("RUN-3") },
                        Self::ENTRY_FORMATION,
                    )
                } else {
                    (span, rule, "expression does not have the expected type")
                };
                // A `?` context fixes the success contract; the fresh error
                // variable is inference machinery, not part of that mismatch.
                let (actual, required) = match (self.partial_result(expected), self.kind(ty)) {
                    (
                        Some((definition, success)),
                        TypeKind::Named {
                            definition: actual,
                            arguments,
                        },
                    ) if definition == actual => match arguments.as_slice() {
                        [GenericArgument::Type(ok), GenericArgument::Type(_)] => (*ok, success),
                        _ => (ty, expected),
                    },
                    _ => (ty, expected),
                };
                let missing_loop_tail = rule.as_str() == "LANG-7"
                    && matches!(&self.input.syntax.tree.expressions[id.0 as usize].kind,
                    ast::ExprKind::Block { statements, tail: None, .. }
                    if statements.last().is_some_and(|statement| matches!(
                        self.input.syntax.tree.statements[statement.0 as usize].kind,
                        ast::StmtKind::While { .. }
                    )));
                let tail_context = if rule.as_str() == "LANG-7" {
                    self.result_tail_context(id, ty, expected)?
                } else {
                    None
                };
                self.type_mismatch(rule, span, message, actual, required);
                self.callback_mode_mismatch(id, actual, required);
                if missing_loop_tail {
                    self.diagnostics.last_mut().unwrap().notes.push(
                        "help: A `while` statement has type `()`, even for `while true`. Add an explicit tail value matching the result, such as `Ok(())`. Use `return` inside the loop for early exits; the enclosing block still needs its tail value.".into(),
                    );
                }
                if let Some((propagation, result)) = tail_context {
                    // Preserve the operand only until `?` has checked its error
                    // channel/mapper. The failed expression still becomes Error;
                    // no implicit wrapping or successful elaboration is introduced.
                    self.result_tail_mismatches
                        .insert(propagation, (result, self.diagnostics.len() - 1));
                } else {
                    ty = self.error();
                }
            }
        }
        self.finish_place(id);
        self.values.insert(id, ty);
        work::count(Counter::BodySourceRows);
        self.source_rows
            .elaborations
            .entry(id)
            .or_insert(self.body.expressions.len());
        self.body.expressions.push(Elaboration {
            construction_destination: None,
            expression: id,
            ty,
            usage,
            instantiated_arguments: self
                .function_value_arguments
                .remove(&id)
                .unwrap_or_default(),
        });
        Ok(())
    }
    fn empty_frame(
        &mut self,
        owner: NameSite,
        parent: Option<usize>,
        result: Option<TypeId>,
        error: Option<TypeId>,
        incomplete: bool,
    ) -> usize {
        let i = self.frames.len();
        self.frames.push(Frame {
            owner,
            parent,
            result,
            error,
            incomplete,
            finalizer: false,
            parameters: vec![],
            locals: BTreeMap::new(),
            loops: vec![],
            blocks: vec![],
            restricted: vec![],
        });
        i
    }
    fn frame(
        &mut self,
        owner: NameSite,
        parent: Option<usize>,
        f: &FunctionType,
        syntax: &ast::FunctionSyntax,
    ) -> usize {
        // Selected mutable state is proved independently by ownership and V
        // flow for every callable. Whole-input
        // Iter and erased carrier creation is proved from actual operands too;
        // field-selected and callable creation results stay fail-closed (307297).
        if self.types.get(f.result).and_then(|t| t.erased_captures.as_ref())
                .is_some_and(|captures| matches!(captures, ErasedCaptures::FromArguments(selectors)
                    if selectors.iter().any(|selector| !selector.fields.is_empty())
                        || !matches!(self.types.get(f.result).map(|row| &row.kind), Some(TypeKind::Builtin { kind: BuiltinType::Iter, .. } | TypeKind::Named { .. })) ))
        {
            let attribute = syntax.attributes.iter().find(|a| {
                matches!(
                    a.arguments.as_slice(),
                    [ast::AttributeArgument::StatePromise(_)]
                )
            });
            let span = attribute.map_or_else(
                || match owner {
                    NameSite::Item => self.input.syntax.item.origin.span,
                    NameSite::Expression(e) => self.span(e),
                    NameSite::Statement(s) => self.input.syntax.tree.statements[s.0 as usize].origin.span,
                    NameSite::Type(t) => self.input.syntax.tree.types[t.0 as usize].origin.span,
                    NameSite::Pattern(p) => self.input.syntax.tree.patterns[p.0 as usize].origin.span,
                },
                |a| a.origin.span,
            );
            self.report(const { RuleId::of("CLO-4") }, span, "this declared capture promise is not yet provable; executable flow proofs are pending");
        }
        let mut scopes = vec![self.site(owner)];
        let mut ancestor = parent;
        while let Some(index) = ancestor {
            scopes.push(self.site(self.frames[index].owner));
            ancestor = self.frames[index].parent;
        }
        let result = bind_parameter_scopes(self.types, f.result, &scopes)
            .expect("checked lexical result scope");
        let error = f.error.map(|t| {
            bind_parameter_scopes(self.types, t, &scopes).expect("checked lexical error scope")
        });
        let frame = self.empty_frame(owner.clone(), parent, Some(result), error, f.incomplete);
        self.body
            .resolution
            .generic_declarations
            .insert(self.site(owner), f.generics.to_vec());
        for (i, p) in f.parameters.iter().enumerate() {
            let ty = bind_parameter_scopes(self.types, p.ty, &scopes)
                .expect("checked lexical parameter scope");
            self.frames[frame].parameters.push(ty);
            if syntax.parameters.get(i).is_some_and(|p| p.name.is_some()) {
                self.body.bindings.push(BindingDecision {
                    owner: owner.clone(),
                    binding: ResolvedName::Parameter(ParamId(i as u32)),
                    site: owner.clone(),
                    ty,
                    usage: passing(p.passing),
                    mutable: p.passing == ast::PassingMode::Mut,
                });
            }
        }
        for contract in syntax.contracts.iter().rev() {
            let boolean = self.primitive(Primitive::Bool);
            self.expected_sites.insert(
                contract.expression,
                (
                    self.span(contract.expression),
                    const { RuleId::of("FACT-1") },
                ),
            );
            self.work
                .push(Work::Enter(contract.expression, Some(boolean), frame));
        }
        frame
    }
    fn final_return(&mut self, body: ast::ExprId) {
        let expr = &self.input.syntax.tree.expressions[body.0 as usize];
        let last = match &expr.kind {
            ast::ExprKind::Block {
                tail, statements, ..
            } => tail.or_else(|| {
                statements.last().and_then(|s| {
                    match self.input.syntax.tree.statements[s.0 as usize].kind {
                        ast::StmtKind::Expr(e) => Some(e),
                        _ => None,
                    }
                })
            }),
            _ => Some(body),
        };
        if let Some(e) = last {
            if matches!(
                self.input.syntax.tree.expressions[e.0 as usize].kind,
                ast::ExprKind::Return(_)
            ) {
                self.report(
                    const { RuleId::of("LANG-11") },
                    self.span(e),
                    "return cannot be the final statement of a function body",
                );
            }
        }
    }
    fn schedule(
        &mut self,
        id: ast::ExprId,
        expected: Option<TypeId>,
        frame: usize,
        children: Vec<(ast::ExprId, Option<TypeId>)>,
    ) {
        self.work.push(Work::Finish(id, expected, frame));
        for (child, expected) in children.into_iter().rev() {
            self.work.push(Work::Enter(child, expected, frame));
        }
    }
    fn step(&mut self, work: Work) -> Result<(), QueryError> {
        match work {
            Work::Enter(id, expected, frame) => self.enter(id, expected, frame),
            Work::Finish(id, expected, frame) => self.finish(id, expected, frame),
            Work::Statement(id, frame) => self.statement(id, frame),
            Work::AfterChild(after) => self.after(after),
        }
    }
    fn enter(
        &mut self,
        id: ast::ExprId,
        expected: Option<TypeId>,
        frame: usize,
    ) -> Result<(), QueryError> {
        use ast::ExprKind as E;
        let kind = self.input.syntax.tree.expressions[id.0 as usize]
            .kind
            .clone();
        match kind {
            E::Literal(lit) => {
                let ty = self.literal(&lit, expected, self.span(id), false);
                self.put(id, ty, expected, UseKind::Copy)?;
            }
            E::With { .. } => {
                self.report(
                    const { RuleId::of("GEN-6") },
                    self.span(id),
                    "a `with` clause is followed by the call's arguments",
                );
                let error = self.error();
                self.put(id, error, expected, UseKind::Borrow)?;
            }
            E::Path(_)
            | E::Formatter(_)
            | E::Heap
            | E::Result
            | E::Shape(_)
            | E::Todo(_)
            | E::Error
            | E::Break { value: None, .. }
            | E::Continue(_) => self.finish(id, expected, frame)?,
            E::Break {
                label,
                value: Some(value),
            } => {
                let target = self.frames[frame]
                    .blocks
                    .iter()
                    .rev()
                    .find(|b| Some(b.label) == label)
                    .and_then(|b| b.ty);
                self.schedule(id, expected, frame, vec![(value, target)]);
            }
            E::Call { callee, arguments }
                if let ast::ExprKind::Formatter(step) =
                    self.input.syntax.tree.expressions[callee.0 as usize].kind =>
            {
                self.start_format_step(id, callee, step, &arguments, expected, frame)?;
            }
            E::Call { callee, .. } => {
                self.work
                    .push(Work::AfterChild(After::CallCallee(id, expected, frame)));
                if let ast::ExprKind::With { callee: base, .. } =
                    self.input.syntax.tree.expressions[callee.0 as usize].kind
                    && let ast::ExprKind::Path(path) = self.input.syntax.tree.expressions
                        [base.0 as usize]
                        .kind
                        .clone()
                {
                    // GEN-6: the call checks against the declaration with its
                    // binders' static actuals; a closed clause's target is the
                    // specialization.
                    let t = self.path_type(base, &path, expected, frame, true, true)?;
                    self.put(base, t, None, UseKind::Borrow)?;
                    self.put(callee, t, None, UseKind::Borrow)?;
                } else if let ast::ExprKind::Path(path) = self.input.syntax.tree.expressions
                    [callee.0 as usize]
                    .kind
                    .clone()
                {
                    let t = self.path_type(callee, &path, expected, frame, true, true)?;
                    self.put(callee, t, None, UseKind::Borrow)?;
                } else {
                    self.work.push(Work::Enter(callee, None, frame));
                }
            }
            E::Binary { op, left, right } => {
                if matches!(op, ast::BinaryOp::Range | ast::BinaryOp::RangeInclusive)
                    && !self.range_sources.contains(&id)
                {
                    // Spec #57: ranges have a direct loop-source contract,
                    // but no first-class value contract. Check independent
                    // operands while withholding the invalid value's context.
                    self.report(
                        const { RuleId::of("SEM-4") },
                        self.span(id),
                        "a range is permitted only as a direct for source",
                    );
                    let error = self.error();
                    let hint = self.hint(left, frame)?.or(self.hint(right, frame)?);
                    self.put(id, error, expected, UseKind::Borrow)?;
                    self.work
                        .push(Work::Enter(right, Some(hint.unwrap_or(error)), frame));
                    self.work
                        .push(Work::Enter(left, Some(hint.unwrap_or(error)), frame));
                    return Ok(());
                }
                let bool_op = matches!(op, ast::BinaryOp::And | ast::BinaryOp::Or);
                let comparison = matches!(
                    op,
                    ast::BinaryOp::Eq
                        | ast::BinaryOp::Ne
                        | ast::BinaryOp::Lt
                        | ast::BinaryOp::Le
                        | ast::BinaryOp::Gt
                        | ast::BinaryOp::Ge
                );
                let hint = if bool_op {
                    Some(self.primitive(Primitive::Bool))
                } else {
                    let e = if comparison { None } else { expected };
                    let hint = self.hint(left, frame)?.or(self.hint(right, frame)?).or(e);
                    hint.map(|t| self.compared_operand_type(op, t))
                };
                self.work
                    .push(Work::AfterChild(After::Binary(id, expected, frame)));
                self.work.push(Work::Enter(left, hint, frame));
            }
            E::Tuple(children) => {
                let wanted = expected.map(|t| self.kind(t));
                let cs = children
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        (
                            *c,
                            match &wanted {
                                Some(TypeKind::Tuple(ts)) => ts
                                    .get(i)
                                    .copied()
                                    .map(|part| self.placed_part_type(expected.unwrap(), part)),
                                _ => None,
                            },
                        )
                    })
                    .collect();
                self.schedule(id, expected, frame, cs);
            }
            E::Sequence(children) => {
                let element = expected
                    .and_then(|t| self.element(t).map(|part| self.placed_part_type(t, part)));
                if expected.is_none()
                    && !children.is_empty()
                    && !self
                        .input
                        .syntax
                        .tree
                        .repeats
                        .iter()
                        .any(|r| r.sequence == id)
                {
                    self.work
                        .push(Work::AfterChild(After::SequenceFirst(id, frame)));
                    self.work.push(Work::Enter(children[0], None, frame));
                } else {
                    self.schedule(
                        id,
                        expected,
                        frame,
                        children.into_iter().map(|c| (c, element)).collect(),
                    );
                }
            }
            E::Unary {
                op: ast::UnaryOp::Neg,
                value,
            } if matches!(
                self.input.syntax.tree.expressions[value.0 as usize].kind,
                E::Literal(ast::Literal::Integer(_))
            ) =>
            {
                let E::Literal(lit) = self.input.syntax.tree.expressions[value.0 as usize]
                    .kind
                    .clone()
                else {
                    unreachable!()
                };
                let ty = self.literal(&lit, expected, self.span(id), true);
                self.put(value, ty, None, UseKind::Copy)?;
                self.put(id, ty, expected, UseKind::Copy)?;
            }
            E::Unary { op, value } => {
                let e = match op {
                    ast::UnaryOp::Not => Some(self.primitive(Primitive::Bool)),
                    ast::UnaryOp::Box => expected.and_then(|t| match self.kind(t) {
                        TypeKind::Box(t) => Some(t),
                        _ => None,
                    }),
                    _ => expected,
                };
                self.schedule(id, expected, frame, vec![(value, e)]);
            }
            E::Before(value) | E::Comptime(value) => {
                self.schedule(id, expected, frame, vec![(value, expected)])
            }
            E::Try { value, .. } => {
                let operand = if let (Some(ok), Some(result)) =
                    (expected, self.frames[frame].result)
                    && let Some((definition, _, _)) = self.result_parts(result)?
                {
                    // The try expression owns a fresh inference scope, separate
                    // from every declared generic binder and enclosing error.
                    let variable =
                        TypeParameterRef::body(self.site(NameSite::Expression(id)), TypeParamId(0));
                    self.inference_variables.insert(variable);
                    let error = self.ty(TypeKind::Parameter(variable));
                    Some(self.ty(TypeKind::Named {
                        definition: DefId::intern(&definition),
                        arguments:
                            vec![GenericArgument::Type(ok), GenericArgument::Type(error)].into(),
                    }))
                } else {
                    None
                };
                self.schedule(id, expected, frame, vec![(value, operand)]);
            }
            E::Field { base, .. } => self.schedule(id, expected, frame, vec![(base, None)]),
            E::Index { base, index } => {
                let index_name = self.input_name(index).map(|(_, name)| name.clone());
                let type_argument = match index_name {
                    Some(ResolvedName::Primitive(_) | ResolvedName::TypeParameter(_)) => true,
                    Some(ResolvedName::Definition(path)) => {
                        matches!(self.snapshot(&path)?.kind, SignatureKind::Type(_))
                    }
                    _ => false,
                };
                if type_argument {
                    // LANG-2: postfix brackets take a value index. Recognize a
                    // type through resolution, before its u64 expectation can
                    // produce a secondary LANG-7 mismatch.
                    self.expression_type_argument_diagnostic(id);
                    let error = self.error();
                    self.put(index, error, None, UseKind::Borrow)?;
                    // The bracketed name is typed as the callee it was meant to
                    // instantiate, so this fault is reported once.
                    if let ast::ExprKind::Path(path) = self.input.syntax.tree.expressions
                        [base.0 as usize]
                        .kind
                        .clone()
                    {
                        let t = self.path_type(base, &path, None, frame, true, true)?;
                        self.put(base, t, None, UseKind::Borrow)?;
                        self.schedule(id, expected, frame, vec![]);
                    } else {
                        self.schedule(id, expected, frame, vec![(base, None)]);
                    }
                } else {
                    let u64 = self.primitive(Primitive::U64);
                    self.schedule(id, expected, frame, vec![(base, None), (index, Some(u64))]);
                }
            }
            E::Block {
                label: Some(label),
                statements,
                tail,
            } => {
                self.check_label_shadow(label, self.span(id), frame);
                let loops = self.frames[frame].loops.len();
                self.frames[frame].blocks.push(LabelledBlock {
                    label,
                    loops,
                    ty: expected,
                    tail_checked: false,
                });
                self.work.push(Work::Finish(id, expected, frame));
                if let Some(t) = tail {
                    self.work
                        .push(Work::AfterChild(After::LabelledTail(t, expected, frame)));
                }
                for stmt in statements.into_iter().rev() {
                    self.work.push(Work::Statement(stmt, frame));
                }
            }
            E::Block {
                statements, tail, ..
            } => {
                self.work.push(Work::Finish(id, expected, frame));
                if let Some(t) = tail {
                    if let Some(mut site) = self.expected_sites.get(&id).cloned() {
                        if site.1.as_str() == "LANG-2" {
                            site.0 = self.span(t);
                        }
                        self.expected_sites.entry(t).or_insert(site);
                    }
                    self.work.push(Work::Enter(t, expected, frame));
                }
                for stmt in statements.into_iter().rev() {
                    self.work.push(Work::Statement(stmt, frame));
                }
            }
            E::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let bool_ty = self.primitive(Primitive::Bool);
                if self
                    .expected_sites
                    .get(&id)
                    .is_some_and(|(_, rule)| rule.as_str() == "LANG-2")
                {
                    for branch in std::iter::once(then_branch).chain(else_branch) {
                        self.expected_sites
                            .insert(branch, (self.span(branch), const { RuleId::of("LANG-2") }));
                    }
                }
                let branch_expected = if else_branch.is_some() {
                    expected
                } else {
                    Some(self.ty(TypeKind::Tuple(vec![].into())))
                };
                self.work.push(Work::Finish(id, expected, frame));
                if else_branch.is_some() {
                    self.work
                        .push(Work::AfterChild(After::IfThen(id, expected, frame)));
                }
                self.work
                    .push(Work::Enter(then_branch, branch_expected, frame));
                self.work.push(Work::Enter(condition, Some(bool_ty), frame));
            }
            E::Match { value, .. } => {
                self.work
                    .push(Work::AfterChild(After::Match(id, expected, frame)));
                self.work.push(Work::Enter(value, None, frame));
            }
            E::Return(value) => {
                let result = self.frames[frame].result;
                if self.frames[frame].finalizer {
                    if let Some(value) = value {
                        self.expected_sites
                            .insert(value, (self.span(id), const { RuleId::of("DROP-3") }));
                    }
                }
                self.schedule(
                    id,
                    expected,
                    frame,
                    value.into_iter().map(|v| (v, result)).collect(),
                );
            }
            E::Closure { signature, body } => {
                let mut f = self.function_syntax(&signature, frame, NameSite::Expression(id))?;
                let inferred = signature.result.is_none();
                // A contextual noalloc callable promises the same effect even
                // without a literal attribute. P checks this closure body under
                // that promise before the driver publishes it (CLO-2, MEM-5).
                if expected.is_some_and(|t| {
                    matches!(self.kind(t), TypeKind::Function(f) if f.allocation == Allocation::NoAlloc)
                }) {
                    f.allocation = Allocation::NoAlloc;
                }
                if let Some(TypeKind::Function(wanted)) = expected.map(|t| self.kind(t)) {
                    if inferred {
                        f.result = wanted.result;
                        f.error = wanted.error;
                    } else if let Some(selection @ ErasedCaptures::FromArguments(_)) = self
                        .types
                        .get(wanted.result)
                        .and_then(|row| row.erased_captures.clone())
                    {
                        // A contextual `@captures` selection is the literal's
                        // own creation promise, proved by its body.
                        let mut row = self.types.get(f.result).unwrap().clone();
                        if row.erased_captures.is_none() {
                            row.erased_captures = Some(selection);
                            f.result = self.types.intern(row);
                        }
                    }
                    // A contextual state promise is the literal's own promise,
                    // proved by its body like a written header's.
                    if f.state_promises.is_empty()
                        && !wanted.state_promises.is_empty()
                        && wanted.parameters.len() == f.parameters.len()
                    {
                        let mut parameters = f.parameters.to_vec();
                        apply_state_promises(self.types, &mut parameters, &wanted.state_promises);
                        f.parameters = parameters.into();
                        f.state_promises = wanted.state_promises.clone();
                    }
                }
                let child = self.frame(NameSite::Expression(id), Some(frame), &f, &signature);
                // MEM-4 grants custody only to a literal in the reviewed core
                // callback slot. Its public callable header stays borrow-only.
                let scope_grant = self.arena_scope_literal(id);
                if scope_grant {
                    for binding in &mut self.body.bindings {
                        if binding.owner == NameSite::Expression(id)
                            && binding.binding == ResolvedName::Parameter(ParamId(0))
                        {
                            binding.usage = UseKind::Take;
                        }
                    }
                }
                if inferred && expected.is_none() {
                    self.frames[child].result = None;
                }
                self.final_return(body);
                self.work.push(Work::AfterChild(After::Closure(
                    id, expected, frame, child, f,
                )));
                self.work
                    .push(Work::Enter(body, self.frames[child].result, child));
            }
        }
        Ok(())
    }
    fn finish(
        &mut self,
        id: ast::ExprId,
        expected: Option<TypeId>,
        frame: usize,
    ) -> Result<(), QueryError> {
        use ast::ExprKind as E;
        let kind = self.input.syntax.tree.expressions[id.0 as usize]
            .kind
            .clone();
        let mut usage = UseKind::Borrow;
        let ty = match kind {
            // Its header names no declaration a value could reference.
            E::Path(_) if self.static_callee(id) => {
                self.report(
                    const { RuleId::of("GEN-6") },
                    self.span(id),
                    "an abstract implementation operation is only ever called",
                );
                self.error()
            }
            E::Path(path) => {
                let ty = self.path_type(id, &path, expected, frame, false, true)?;
                if let TypeKind::Function(f) = self.kind(ty)
                    && !f.generics.is_empty()
                {
                    self.instantiate_function_value(id, ty, f, expected)?
                } else {
                    ty
                }
            }
            E::Tuple(cs) => {
                if cs.iter().any(|c| self.invalid(self.value(*c))) {
                    self.error()
                } else {
                    let actual = self.inferred_aggregate_type(TypeKind::Tuple(
                        cs.into_iter().map(|c| self.value(c)).collect(),
                    ));
                    // An expected tuple type supplies the part contracts, as a
                    // record's declaration does; the operands are then values
                    // for those parts, not the parts' types.
                    if let Some(expected) = expected.filter(|t| {
                        matches!(self.kind(*t), TypeKind::Tuple(_)) && self.compatible(actual, *t)
                    }) {
                        let mut row = self.types.get(actual).unwrap().clone();
                        row.kind = self.kind(expected);
                        if let Some(placement) = self.construction_placement(expected) {
                            row.modes.relation = row.modes.relation.union(&placement);
                        }
                        self.types.intern(row)
                    } else {
                        actual
                    }
                }
            }
            E::Sequence(cs) => {
                let repeat = self
                    .input
                    .syntax
                    .tree
                    .repeats
                    .iter()
                    .any(|r| r.sequence == id);
                if expected.is_none() {
                    self.sequence_prefix_guidance(&cs, repeat);
                }
                if cs.iter().any(|c| self.invalid(self.value(*c))) {
                    self.error()
                } else if let Some(t) = expected.filter(|t| self.element(*t).is_some()) {
                    if let TypeKind::Builtin {
                        kind: BuiltinType::Inline,
                        arguments,
                    } = self.kind(t)
                    {
                        // Sizes compare by equality only (LANG-7), so an
                        // abstract size never equals a literal's length.
                        let length = match arguments.get(1) {
                            Some(GenericArgument::Size(Size::Constant(n))) => Some(*n),
                            _ => None,
                        };
                        if !self
                            .input
                            .syntax
                            .tree
                            .repeats
                            .iter()
                            .any(|r| r.sequence == id)
                            && length != Some(cs.len() as u64)
                        {
                            self.report(
                                const { RuleId::of("LANG-7") },
                                self.span(id),
                                "Inline literal has the wrong length",
                            );
                        }
                    }
                    t
                } else {
                    let (message, help) = if expected.is_some() {
                        (
                            "sequence literal has a context that is not a sequence type",
                            "help: replace the receiving type with the intended complete sequence type, such as `List[u64]`, `Array[u64]`, or `Inline[u64, 0]` for `[]`; Inline's size must equal the literal length",
                        )
                    } else if repeat {
                        (
                            "repeat sequence needs an expected Array or Inline type",
                            Self::REPEAT_SEQUENCE_HELP,
                        )
                    } else if cs.is_empty() {
                        (
                            "empty sequence has no first element to determine its type",
                            "help: annotate the binding, for example `let values: List[u64] = []`, `let values: Array[u64] = []`, or `let values: Inline[u64, 0] = []`; choose the intended element type and sequence kind",
                        )
                    } else {
                        (
                            "first sequence element does not determine a complete element type",
                            Self::SEQUENCE_ANNOTATION_HELP,
                        )
                    };
                    self.report(const { RuleId::of("LANG-2") }, self.span(id), message);
                    self.diagnostics.last_mut().unwrap().notes.push(help.into());
                    self.error()
                }
            }
            E::Unary { op, value } => {
                let t = self.value(value);
                usage = match op {
                    ast::UnaryOp::Take => UseKind::Take,
                    ast::UnaryOp::Copy => UseKind::Copy,
                    ast::UnaryOp::Mut => UseKind::Mut,
                    _ => UseKind::Borrow,
                };
                if self.invalid(t) {
                    t
                } else if op == ast::UnaryOp::Mut && self.readonly_byte_projection(value) {
                    self.report(
                        const { RuleId::of("LANG-5") },
                        self.span(id),
                        "Bytes indexing is read-only",
                    );
                    self.error()
                } else {
                    match op {
                        ast::UnaryOp::Box => {
                            let actual = self.inferred_aggregate_type(TypeKind::Box(t));
                            if let Some(expected) = expected.filter(|goal| {
                                matches!(self.kind(*goal), TypeKind::Box(_))
                                    && self.compatible(actual, *goal)
                            }) {
                                // The expected payload is the construction contract;
                                // operand uniqueness is evidence about the value stored.
                                let mut row = self.types.get(actual).unwrap().clone();
                                row.kind = self.kind(expected);
                                if let Some(placement) = self.construction_placement(expected) {
                                    row.modes.relation = row.modes.relation.union(&placement);
                                }
                                self.types.intern(row)
                            } else {
                                actual
                            }
                        }
                        ast::UnaryOp::Neg | ast::UnaryOp::BitNot => {
                            if !self.numeric(t) || (op == ast::UnaryOp::BitNot && !self.integer(t))
                            {
                                self.report(
                                    const { RuleId::of("LANG-7") },
                                    self.span(id),
                                    "operator requires a sized numeric operand",
                                );
                                self.error()
                            } else {
                                let mut result = self.types.get(t).unwrap().clone();
                                result.brand = Brand::default();
                                self.types.intern(result)
                            }
                        }
                        ast::UnaryOp::Not => {
                            let mut result = self.types.get(t).unwrap().clone();
                            result.brand = Brand::default();
                            self.types.intern(result)
                        }
                        ast::UnaryOp::Copy => {
                            if matches!(self.kind(t), TypeKind::Parameter(_))
                                && !self.bound(t, ast::Bound::Copyable)?
                            {
                                self.report(
                                    const { RuleId::of("GEN-1") },
                                    self.span(id),
                                    "generic copy requires copyable",
                                );
                                self.diagnostics.last_mut().unwrap().obligations =
                                    vec![hirsel_diag::Obligation::CopyProof];
                                self.error()
                            } else {
                                let mut copy = self.types.get(t).unwrap().clone();
                                copy.sendability_requirement = SendabilityRequirement::None;
                                self.types.intern(copy)
                            }
                        }
                        _ => t,
                    }
                }
            }
            E::Binary { op, left, right } => self.binary(id, op, left, right)?,
            E::Field { base, field } => {
                let t = self.value(base);
                if self.invalid(t) {
                    t
                } else {
                    let payload = self.box_payload(t);
                    let (_, repr) = self.representation(payload)?;
                    let name = self.name(field);
                    let found = match self.kind(repr) {
                        TypeKind::Record(fs) => fs
                            .iter()
                            .enumerate()
                            .find(|(_, f)| f.name.as_str() == name)
                            .map(|(i, f)| (FieldId(i as u32), f.ty)),
                        TypeKind::Tuple(ts) => name
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| ts.get(i).map(|t| (FieldId(i as u32), *t))),
                        _ => None,
                    };
                    if let Some((field, part)) = found {
                        self.project_box_place(id, base);
                        self.project_place(id, id, SourceProjectionKind::Field(field));
                        self.projected_type(t, part)
                    } else {
                        self.report(
                            const { RuleId::of("LANG-7") },
                            self.span(id),
                            "field is not available on this type",
                        );
                        if let TypeKind::Record(fields) = self.kind(repr) {
                            let span = self.span(id);
                            // A field expression ends at its identifier token.
                            let name_span =
                                Span::new(span.anchor, span.end - name.len() as u32, span.end);
                            let prefix =
                                match &self.input.syntax.tree.expressions[base.0 as usize].kind {
                                    ast::ExprKind::Path(path) => path
                                        .segments
                                        .iter()
                                        .map(|s| format!("{}.", self.name(*s)))
                                        .collect(),
                                    _ => String::new(),
                                };
                            hirsel_diag::suggest_names(
                                self.diagnostics.last_mut().unwrap(),
                                &name,
                                fields.into_iter().map(|f| f.name.to_string()),
                                &prefix,
                                name_span,
                            );
                        }
                        self.error()
                    }
                }
            }
            E::Index { base, index } => {
                let base_ty = self.value(base);
                let t = self.box_payload(base_ty);
                if self.invalid(t) || self.invalid(self.value(index)) {
                    self.error()
                } else if matches!(self.kind(t), TypeKind::Primitive(Primitive::Bytes)) {
                    self.primitive(Primitive::U8)
                } else if let Some(e) = self.element(t) {
                    self.projected_type(t, e)
                } else {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "indexing requires a sequence",
                    );
                    self.error()
                }
            }
            E::Try { value, map_error } => {
                let success = self.try_type(id, value, map_error.as_ref(), frame)?;
                if let Some((result, diagnostic)) = self.result_tail_mismatches.remove(&id) {
                    if !self.invalid(success) {
                        self.result_tail_help(
                            value,
                            map_error.is_some(),
                            success,
                            result,
                            diagnostic,
                        );
                    }
                    self.error()
                } else {
                    success
                }
            }
            E::Block {
                label: Some(_),
                tail,
                ..
            } => {
                let block = self.frames[frame]
                    .blocks
                    .pop()
                    .expect("entered labelled block");
                let t = tail
                    .map(|t| self.value(t))
                    .unwrap_or_else(|| self.ty(TypeKind::Tuple(vec![].into())));
                match block.ty {
                    _ if self.invalid(t) => self.error(),
                    None => t,
                    Some(ty) if self.invalid(ty) => self.error(),
                    Some(ty) if self.bottom(t) || block.tail_checked || self.compatible(t, ty) => {
                        ty
                    }
                    Some(ty) => {
                        self.type_mismatch(
                            const { RuleId::of("LANG-7") },
                            tail.map_or(self.span(id), |t| self.span(t)),
                            "a labelled block's tail and its break values have different types",
                            t,
                            ty,
                        );
                        self.error()
                    }
                }
            }
            E::Block { tail, .. } => tail
                .map(|t| self.value(t))
                .unwrap_or_else(|| self.ty(TypeKind::Tuple(vec![].into()))),
            E::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let a = self.value(then_branch);
                let b = else_branch
                    .map(|e| self.value(e))
                    .unwrap_or_else(|| self.ty(TypeKind::Tuple(vec![].into())));
                if self.invalid(self.value(condition)) || self.invalid(a) || self.invalid(b) {
                    self.error()
                } else if self.bottom(a) {
                    b
                } else if self.bottom(b) || self.compatible(a, b) {
                    a
                } else {
                    self.type_mismatch(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "branches have different types",
                        b,
                        a,
                    );
                    self.error()
                }
            }
            E::Match { value, arms } => {
                let mut result = None;
                let mut bad = self.invalid(self.value(value));
                for arm in &arms {
                    let t = self.value(arm.body);
                    bad |= self.invalid(t);
                    if !self.bottom(t) {
                        if let Some(a) = result {
                            if !self.compatible(a, t) {
                                bad = true;
                                self.type_mismatch(
                                    const { RuleId::of("LANG-7") },
                                    self.span(id),
                                    "match arms have different types",
                                    t,
                                    a,
                                );
                            }
                        } else {
                            result = Some(t);
                        }
                    }
                }
                if bad {
                    self.error()
                } else {
                    result.unwrap_or_else(|| self.primitive(Primitive::Never))
                }
            }
            E::Return(value) => {
                if let Some(value) = value {
                    if self.frames[frame].result.is_none() && !self.invalid(self.value(value)) {
                        self.frames[frame].result = Some(self.value(value));
                    }
                } else if let Some(result) = self.frames[frame].result {
                    let unit = self.ty(TypeKind::Tuple(vec![].into()));
                    if !self.compatible(unit, result) {
                        self.type_mismatch(
                            const { RuleId::of("LANG-7") },
                            self.span(id),
                            "bare return requires unit result",
                            unit,
                            result,
                        );
                    }
                }
                self.primitive(Primitive::Never)
            }
            E::Break {
                label: Some(label),
                value,
            } if self.frames[frame].blocks.iter().any(|b| b.label == label) => {
                let unit = self.ty(TypeKind::Tuple(vec![].into()));
                let t = value.map_or(unit, |v| self.value(v));
                let block = self.frames[frame]
                    .blocks
                    .iter()
                    .rposition(|b| b.label == label)
                    .expect("matched labelled block");
                match self.frames[frame].blocks[block].ty {
                    _ if self.invalid(t) || self.bottom(t) => {}
                    None => self.frames[frame].blocks[block].ty = Some(t),
                    // A typed operand was checked against `ty` when entered.
                    Some(_) if value.is_some() => {}
                    Some(ty) if self.invalid(ty) || self.compatible(t, ty) => {}
                    Some(ty) => self.type_mismatch(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "a break without a value gives the labelled block ()",
                        t,
                        ty,
                    ),
                }
                self.primitive(Primitive::Never)
            }
            E::Continue(Some(label))
                if self.frames[frame].blocks.iter().any(|b| b.label == label) =>
            {
                self.report(
                    const { RuleId::of("LANG-7") },
                    self.span(id),
                    "continue needs a loop label, and this label names a block",
                );
                self.error()
            }
            E::Break {
                label: Some(label),
                value: Some(_),
            } if self.frames[frame].loops.iter().any(|statement| {
                self.input.syntax.tree.statements[statement.0 as usize]
                    .kind
                    .loop_label()
                    == Some(label)
            }) =>
            {
                self.report(
                    const { RuleId::of("LANG-1") },
                    self.span(id),
                    "only a labelled block takes a break value; a loop has no result",
                );
                self.error()
            }
            E::Break { label: None, .. } | E::Continue(None)
                if self.frames[frame]
                    .blocks
                    .last()
                    .is_some_and(|b| b.loops == self.frames[frame].loops.len())
                    && !self.frames[frame].loops.is_empty() =>
            {
                self.report(
                    const { RuleId::of("LANG-7") },
                    self.span(id),
                    "an unlabelled exit cannot leave a labelled block; name the loop's label",
                );
                self.error()
            }
            E::Break { label, .. } | E::Continue(label) => {
                let found = self.frames[frame].loops.iter().rev().any(|statement| {
                    label.is_none()
                        || self.input.syntax.tree.statements[statement.0 as usize]
                            .kind
                            .loop_label()
                            == label
                });
                if !found {
                    self.report(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "loop control requires a matching enclosing loop in this function",
                    );
                    self.error()
                } else {
                    self.primitive(Primitive::Never)
                }
            }
            E::Before(v) | E::Comptime(v) => self.value(v),
            E::Shape(t) => {
                let syntax = t;
                let t = self.syntax_type(t, frame)?;
                match self.shape_subject(id, t)? {
                    Some(t) => {
                        self.plans.shapes.push((id, syntax, t));
                        self.ty(TypeKind::Builtin {
                            kind: BuiltinType::TypeShape,
                            arguments: vec![GenericArgument::Type(t)].into(),
                        })
                    }
                    None => self.error(),
                }
            }
            E::Heap => {
                let role = self
                    .input
                    .signature
                    .metadata
                    .roles
                    .heap
                    .clone()
                    .ok_or_else(|| {
                        QueryError::Operational(OperationalFailure {
                            operation: "heap value".into(),
                            message: "missing canonical heap operation".into(),
                        })
                    })?;
                let signature = self.snapshot(&role.operation)?;
                let SignatureKind::Function(f) = &signature.kind else {
                    return Err(QueryError::Operational(OperationalFailure {
                        operation: "heap value".into(),
                        message: "heap operation is not callable".into(),
                    }));
                };
                if !f.parameters.is_empty()
                    || !f.generics.is_empty()
                    || f.allocation != Allocation::NoAlloc
                    || f.authority != AuthoritySummary::inert()
                    || !self
                        .types
                        .get(f.result)
                        .is_some_and(|t| t.is_heap_target(&self.input.signature.metadata.roles))
                {
                    return Err(QueryError::Operational(OperationalFailure {
                        operation: "heap value".into(),
                        message: "invalid private heap producer contract".into(),
                    }));
                }
                let result = self.import(f.result)?;
                self.body.calls.push(CheckedCall {
                    expression: id,
                    target: CallTarget::Definition(role.operation),
                    arguments: vec![],
                    generic_arguments: vec![],
                    contextual_contracts: vec![],
                    result,
                    state: CallState::Complete,
                });
                result
            }
            E::Result => self.frames[frame].result.unwrap_or_else(|| self.error()),
            E::Todo(_) => {
                if !self.frames[frame].incomplete {
                    self.report(
                        const { RuleId::of("LANG-6") },
                        self.span(id),
                        "todo requires @incomplete on its enclosing definition",
                    );
                }
                self.primitive(Primitive::Never)
            }
            E::Error => self.error(),
            E::Call { .. } | E::Closure { .. } | E::Literal(_) => {
                unreachable!("specialized continuation finishes this expression")
            }
            E::Formatter(_) => unreachable!("the parser builds a formatter only as a callee"),
            E::With { .. } => unreachable!("a `with` clause is entered only as a callee"),
        };
        if usage == UseKind::Borrow && self.bound(ty, ast::Bound::Plain)? {
            usage = UseKind::Copy;
        }
        self.put(id, ty, expected, usage)
    }
    fn enter_loop(&mut self, id: ast::StmtId, frame: usize) {
        if let Some(label) = self.input.syntax.tree.statements[id.0 as usize]
            .kind
            .loop_label()
        {
            let span = self.input.syntax.tree.statements[id.0 as usize].origin.span;
            self.check_label_shadow(label, span, frame);
        }
        self.frames[frame].loops.push(id);
    }
    /// Loop and block labels share one namespace and never shadow (LANG-11).
    fn check_label_shadow(&mut self, label: Symbol, span: Span, frame: usize) {
        let f = &self.frames[frame];
        if f.blocks.iter().any(|b| b.label == label)
            || f.loops.iter().any(|outer| {
                self.input.syntax.tree.statements[outer.0 as usize]
                    .kind
                    .loop_label()
                    == Some(label)
            })
        {
            self.report(
                const { RuleId::of("LANG-7") },
                span,
                "a label shadows an enclosing loop or block label",
            );
        }
    }
    fn statement(&mut self, id: ast::StmtId, frame: usize) -> Result<(), QueryError> {
        use ast::StmtKind as S;
        let stmt = self.input.syntax.tree.statements[id.0 as usize].clone();
        match stmt.kind {
            S::Let {
                ty,
                value,
                otherwise,
                ..
            } => {
                let e = ty.map(|t| self.syntax_type(t, frame)).transpose()?;
                self.work.push(Work::AfterChild(After::Let(id, frame)));
                if let Some(otherwise) = otherwise {
                    self.work.push(Work::Enter(otherwise, None, frame));
                }
                self.work.push(Work::Enter(value, e, frame));
            }
            S::Assign { place, .. } => {
                self.work
                    .push(Work::AfterChild(After::AssignPlace(id, frame)));
                self.work.push(Work::Enter(place, None, frame));
            }
            S::Expr(e) => self.work.push(Work::Enter(e, None, frame)),
            S::While {
                condition, body, ..
            } => {
                self.enter_loop(id, frame);
                let b = self.primitive(Primitive::Bool);
                let unit = self.ty(TypeKind::Tuple(vec![].into()));
                self.work.push(Work::AfterChild(After::LoopEnd(
                    frame,
                    self.frames[frame].restricted.len(),
                )));
                self.work.push(Work::Enter(body, Some(unit), frame));
                self.work.push(Work::Enter(condition, Some(b), frame));
            }
            S::For { source, .. } => {
                self.range_sources.insert(source);
                self.work.push(Work::AfterChild(After::For(id, frame)));
                self.work.push(Work::Enter(source, None, frame));
            }
            S::Function {
                signature, body, ..
            } => {
                let f = self.function_syntax(&signature, frame, NameSite::Statement(id))?;
                let child = self.frame(NameSite::Statement(id), Some(frame), &f, &signature);
                self.final_return(body);
                self.work.push(Work::AfterChild(After::LocalFunction(
                    id,
                    frame,
                    child,
                    f.clone(),
                )));
                self.work
                    .push(Work::Enter(body, self.frames[child].result, child));
            }
        }
        Ok(())
    }
    fn after(&mut self, after: After) -> Result<(), QueryError> {
        match after {
            After::SequenceFirst(id, frame) => {
                let ast::ExprKind::Sequence(children) = self.input.syntax.tree.expressions
                    [id.0 as usize]
                    .kind
                    .clone()
                else {
                    unreachable!()
                };
                let first = self.value(children[0]);
                // No lookahead or recovery from a later element: only the
                // completed first expression may establish the contract.
                let mut pending = vec![first];
                let mut seen = std::collections::HashSet::new();
                let mut complete = !self.bottom(first);
                while let Some(ty) = pending.pop() {
                    if seen.insert(ty) {
                        complete &= !self.invalid(ty) && !self.inference_variable(ty);
                        pending.extend(self.kind(ty).graph_edges().into_iter().map(|e| e.ty));
                    }
                }
                let sequence = if complete {
                    // Builtin identity selects core List even when the spelling
                    // is shadowed. Normalize value access just as a generic
                    // type argument does; nested contracts and brands survive.
                    let arenas = self.arena_places(first);
                    let element = generic_type_argument(self.types, first, |o| arenas.contains(o))
                        .map_err(|error| {
                            QueryError::Operational(OperationalFailure {
                                operation: "sequence element type".into(),
                                message: format!("{error:?}"),
                            })
                        })?;
                    Some(self.inferred_aggregate_type(TypeKind::Builtin {
                        kind: BuiltinType::List,
                        arguments: vec![GenericArgument::Type(element)].into(),
                    }))
                } else {
                    None
                };
                let element = sequence.and_then(|t| self.element(t));
                self.schedule(
                    id,
                    sequence,
                    frame,
                    children.into_iter().skip(1).map(|c| (c, element)).collect(),
                );
            }
            After::Binary(id, expected, frame) => {
                let ast::ExprKind::Binary { op, left, right } =
                    self.input.syntax.tree.expressions[id.0 as usize].kind
                else {
                    unreachable!()
                };
                let t = self.value(left);
                self.work.push(Work::Finish(id, expected, frame));
                let rule = if self.integer(t) {
                    const { RuleId::of("SEM-1") }
                } else if matches!(
                    self.kind(t),
                    TypeKind::Primitive(Primitive::F32 | Primitive::F64)
                ) {
                    const { RuleId::of("SEM-2") }
                } else {
                    const { RuleId::of("LANG-7") }
                };
                self.expected_sites.insert(right, (self.span(id), rule));
                let t = self.compared_operand_type(op, t);
                self.work.push(Work::Enter(right, Some(t), frame));
            }
            After::CallCallee(id, expected, frame) => self.start_call(id, expected, frame, 0)?,
            After::CallArgument(call, arg) => self.call_argument(call, arg)?,
            After::FormatAppend(id, expected, frame) => self.format_append(id, expected, frame)?,
            After::Let(id, frame) => {
                let ast::StmtKind::Let {
                    mutable,
                    pattern,
                    value,
                    ty: annotation,
                    otherwise,
                } = self.input.syntax.tree.statements[id.0 as usize].kind
                else {
                    unreachable!()
                };
                let t = self.value(value);
                // LANG-13: a let-else borrows or takes as match does, so an
                // unmarked binding place is inspected in place, not moved.
                let inspected = otherwise.is_some()
                    && !matches!(
                        self.input.syntax.tree.expressions[value.0 as usize].kind,
                        ast::ExprKind::Unary { .. }
                    )
                    && self
                        .source_place(value)
                        .is_some_and(|p| matches!(p.root, SourceRoot::Binding { .. }));
                let usage = if inspected && !self.bound(t, ast::Bound::Plain)? {
                    UseKind::Borrow
                } else {
                    self.binding_use(value, t, frame)?
                };
                let written = if let Some(annotation) = annotation {
                    let mut written = self.written_modes(annotation);
                    let declared = self.syntax_type(annotation, frame)?;
                    written.sendable |= self.types.get(declared).unwrap().sendability_requirement
                        == SendabilityRequirement::Sendable;
                    Some(written)
                } else {
                    None
                };
                let mut received = t;
                if let Some(annotation) = annotation {
                    let declared = self.syntax_type(annotation, frame)?;
                    received = self.written_promises(received, declared);
                }
                if let Some(annotation) = annotation.filter(|_| written.unwrap().relation) {
                    let declared = self.syntax_type(annotation, frame)?;
                    let mut row = self.types.get(t).unwrap().clone();
                    row.modes.relation = self
                        .placement(&row.modes.relation)
                        .union(&self.types.get(declared).unwrap().modes.relation);
                    received = self.types.intern(row);
                }
                // The initializer retains its current modes; written locality
                // belongs to the receiving binding and its checked storage.
                let first_binding = self.body.bindings.len();
                self.pattern(
                    pattern,
                    received,
                    usage,
                    mutable,
                    otherwise.is_none(),
                    written,
                    frame,
                )?;
                if let Some(annotation) = annotation {
                    // Ownership determines whether reads through aliases are
                    // borrowed-only. Defer potential cursor covers to it, but
                    // retain MODE-1 for patterns without eligible bindings.
                    let mutable: Vec<_> = self.body.bindings[first_binding..]
                        .iter()
                        .filter(|b| b.mutable)
                        .map(|b| b.ty)
                        .collect();
                    self.environment_relation(annotation, "a binding", &mutable)?;
                }
                self.joined_bindings_own(pattern);
                if let Some(otherwise) = otherwise {
                    self.let_else(pattern, received, otherwise)?;
                }
                // LANG-15/OWN-8: only a borrowed receipt aliases the parameter.
                // Plain reads have already selected Copy in binding_use.
                if usage == UseKind::Borrow
                    && let Some((_, r @ ResolvedName::Parameter(_))) = self.input_name(value)
                {
                    if let Some(ty) = self.lookup_binding(value, r, frame) {
                        if self
                            .body
                            .bindings
                            .iter()
                            .any(|b| b.binding == *r && b.ty == ty && b.mutable)
                        {
                            for b in &self.body.bindings {
                                if b.site == NameSite::Pattern(pattern) {
                                    if let ResolvedName::Local(l) = b.binding {
                                        self.mut_aliases.insert(l);
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(e) = self.elaboration(value) {
                    e.usage = usage;
                }
            }
            After::AssignPlace(id, frame) => {
                let ast::StmtKind::Assign { place, value, .. } =
                    self.input.syntax.tree.statements[id.0 as usize].kind
                else {
                    unreachable!()
                };
                let t = self.value(place);
                self.work.push(Work::AfterChild(After::Assign(id, frame)));
                self.work.push(Work::Enter(value, Some(t), frame));
            }
            After::Assign(id, frame) => {
                let stmt = &self.input.syntax.tree.statements[id.0 as usize];
                let ast::StmtKind::Assign { place, op, value } = stmt.kind else {
                    unreachable!()
                };
                if !self.invalid(self.value(place)) && !self.invalid(self.value(value)) {
                    let access = self.mutable_place(place, frame, op.0.is_none());
                    if let Some(rule) = access {
                        self.report(
                            rule,
                            stmt.origin.span,
                            "assignment requires a visible mutable place",
                        );
                    }
                    if let Some(op) = op.0 {
                        self.binary(place, op, place, value)?;
                    }
                }
            }
            After::Match(id, expected, frame) => {
                let ast::ExprKind::Match { value, arms } = self.input.syntax.tree.expressions
                    [id.0 as usize]
                    .kind
                    .clone()
                else {
                    unreachable!()
                };
                let ty = self.value(value);
                for arm in &arms {
                    self.pattern(arm.pattern, ty, UseKind::Borrow, false, false, None, frame)?;
                }
                if !self.invalid(ty) && !self.exhaustive(ty, &arms)? {
                    let notes = self.missing_patterns(ty, &arms)?;
                    self.report(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "match does not cover every value",
                    );
                    self.diagnostics.last_mut().unwrap().notes.extend(notes);
                }
                if !self.invalid(ty) {
                    self.unreachable_patterns(ty, &arms)?;
                }
                // LANG-13: an if-let without else is an if without else.
                let mut arm_expected = expected;
                let conditions = &self.input.syntax.tree.conditions;
                if let Some(condition) = conditions.iter().copied().find(|c| c.matching() == id) {
                    self.let_condition(condition, ty, arms[0].pattern)?;
                    if let ast::LetCondition::If {
                        otherwise: false, ..
                    } = condition
                    {
                        arm_expected = Some(self.ty(TypeKind::Tuple(vec![].into())));
                    }
                }
                self.work.push(Work::Finish(id, expected, frame));
                self.work.push(Work::AfterChild(After::MatchArm(
                    id,
                    0,
                    arm_expected,
                    frame,
                    self.expected_sites
                        .get(&id)
                        .is_some_and(|(_, rule)| rule.as_str() == "LANG-2"),
                )));
            }
            After::MatchArm(id, index, expected, frame, prefix) => {
                let ast::ExprKind::Match { arms, .. } =
                    &self.input.syntax.tree.expressions[id.0 as usize].kind
                else {
                    unreachable!()
                };
                let hint = expected.or_else(|| {
                    index.checked_sub(1).and_then(|previous| {
                        let ty = self.value(arms[previous].body);
                        (!self.invalid(ty) && !self.bottom(ty)).then_some(ty)
                    })
                });
                if let Some(arm) = arms.get(index) {
                    let (body, guard) = (arm.body, arm.guard);
                    self.work.push(Work::AfterChild(After::MatchArm(
                        id,
                        index + 1,
                        hint,
                        frame,
                        prefix || (expected.is_none() && hint.is_some()),
                    )));
                    if prefix || (expected.is_none() && hint.is_some()) {
                        self.expected_sites
                            .insert(body, (self.span(body), const { RuleId::of("LANG-2") }));
                    }
                    self.work.push(Work::Enter(body, hint, frame));
                    if let Some(guard) = guard {
                        let bool_ty = self.primitive(Primitive::Bool);
                        self.work.push(Work::Enter(guard, Some(bool_ty), frame));
                    }
                }
            }
            After::IfThen(id, expected, frame) => {
                let ast::ExprKind::If {
                    then_branch,
                    else_branch: Some(other),
                    ..
                } = self.input.syntax.tree.expressions[id.0 as usize].kind
                else {
                    unreachable!()
                };
                let hint = expected.or_else(|| {
                    let ty = self.value(then_branch);
                    (!self.invalid(ty) && !self.bottom(ty)).then_some(ty)
                });
                if expected.is_none() && hint.is_some() {
                    self.expected_sites
                        .insert(other, (self.span(other), const { RuleId::of("LANG-2") }));
                }
                self.work.push(Work::Enter(other, hint, frame));
            }
            After::For(id, frame) => {
                let ast::StmtKind::For {
                    pattern,
                    source,
                    body,
                    ..
                } = self.input.syntax.tree.statements[id.0 as usize].kind
                else {
                    unreachable!()
                };
                let t = self.value(source);
                let e = self.iteration_type(source, t)?;
                let marker = match self.input.syntax.tree.expressions[source.0 as usize].kind {
                    ast::ExprKind::Unary {
                        op: ast::UnaryOp::Mut,
                        ..
                    } => UseKind::Mut,
                    ast::ExprKind::Unary {
                        op: ast::UnaryOp::Take,
                        ..
                    } => UseKind::Take,
                    _ => UseKind::Borrow,
                };
                let iterator = matches!(
                    self.kind(t),
                    TypeKind::Builtin {
                        kind: BuiltinType::Iter,
                        ..
                    }
                ) && !self.ranges.contains_key(&source);
                let binding_use = if iterator { UseKind::Take } else { marker };
                let previous = self.frames[frame].restricted.len();
                let protocol = match &self.input.syntax.tree.expressions[source.0 as usize].kind {
                    ast::ExprKind::Binary {
                        op: ast::BinaryOp::Range | ast::BinaryOp::RangeInclusive,
                        ..
                    } => ForProtocol::Range {
                        inclusive: matches!(
                            self.input.syntax.tree.expressions[source.0 as usize].kind,
                            ast::ExprKind::Binary {
                                op: ast::BinaryOp::RangeInclusive,
                                ..
                            }
                        ),
                    },
                    _ => match self.kind(t) {
                        TypeKind::Builtin {
                            kind: BuiltinType::Iter,
                            ..
                        } => ForProtocol::Iterator,
                        TypeKind::Builtin {
                            kind: kind @ (BuiltinType::Map | BuiltinType::SortedMap),
                            ..
                        } => ForProtocol::Map(kind),
                        TypeKind::Builtin {
                            kind: BuiltinType::Set,
                            ..
                        } => ForProtocol::Set,
                        TypeKind::Builtin {
                            kind:
                                kind @ (BuiltinType::Array | BuiltinType::List | BuiltinType::Inline),
                            ..
                        } if marker == UseKind::Take => ForProtocol::ConsumingSequence(kind),
                        TypeKind::Builtin { kind, .. } => ForProtocol::Sequence(kind),
                        TypeKind::Primitive(Primitive::Bytes) => ForProtocol::Bytes,
                        TypeKind::Named { definition, .. } => {
                            ForProtocol::BorrowedSource(definition.path().clone())
                        }
                        _ => ForProtocol::Iterator,
                    },
                };
                if marker == UseKind::Mut && matches!(protocol, ForProtocol::Bytes) {
                    self.report(
                        const { RuleId::of("SEM-4") },
                        self.span(source),
                        "Bytes loops are read-only; borrow the source or consume it with take",
                    );
                    return Ok(());
                }
                let cursor = self.cursor_plan(id, source, t, e, &protocol)?;
                let e = cursor.as_ref().map_or(e, |cursor| cursor.step.item);
                // A nominal borrowed association returns owned Some payloads.
                // Receiver borrowing and yield ownership are independent.
                let binding_use = if cursor.as_ref().is_some_and(|c| c.association.is_some()) {
                    UseKind::Take
                } else {
                    binding_use
                };
                self.pattern(
                    pattern,
                    e,
                    binding_use,
                    marker == UseKind::Mut && !iterator,
                    true,
                    None,
                    frame,
                )?;
                if marker == UseKind::Mut && matches!(protocol, ForProtocol::Map(_)) {
                    self.mutable_map_pattern(pattern);
                }
                self.joined_bindings_own(pattern);
                let kind = if let Some(cursor) = cursor {
                    ForPlanKind::Cursor { cursor }
                } else {
                    ForPlanKind::Direct {
                        statement: id,
                        execution: self.loop_execution(source, t, e, &protocol)?,
                    }
                };
                self.plans.loops.push(ForPlan { protocol, kind });
                if marker == UseKind::Mut && !iterator {
                    if let TypeKind::Named { definition, .. } = self.kind(e) {
                        let s = self.snapshot(&definition)?;
                        if matches!(&s.kind,SignatureKind::Type(td) if td.fact.is_some()) {
                            self.report(
                                const { RuleId::of("SEM-4") },
                                self.input.syntax.tree.statements[id.0 as usize].origin.span,
                                "mutable iteration element may not carry facts",
                            );
                        }
                    }
                    let base = match self.input.syntax.tree.expressions[source.0 as usize].kind {
                        ast::ExprKind::Unary { value, .. } => value,
                        _ => source,
                    };
                    if let Some((_, r)) = self.input_name(base) {
                        self.frames[frame].restricted.push((
                            r.clone(),
                            self.input.syntax.tree.statements[id.0 as usize].origin.span,
                        ));
                    }
                }
                self.enter_loop(id, frame);
                let unit = self.ty(TypeKind::Tuple(vec![].into()));
                self.work
                    .push(Work::AfterChild(After::LoopEnd(frame, previous)));
                self.work.push(Work::Enter(body, Some(unit), frame));
            }
            After::LabelledTail(tail, expected, frame) => {
                let block = self.frames[frame]
                    .blocks
                    .last_mut()
                    .expect("entered labelled block");
                block.tail_checked = block.ty.is_some();
                let expected = block.ty.or(expected);
                self.work.push(Work::Enter(tail, expected, frame));
            }
            After::LoopEnd(frame, restricted) => {
                self.frames[frame].loops.pop();
                self.frames[frame].restricted.truncate(restricted);
            }
            After::Closure(id, expected, _parent, child, mut f) => {
                let ast::ExprKind::Closure { body, .. } =
                    self.input.syntax.tree.expressions[id.0 as usize].kind
                else {
                    unreachable!()
                };
                let result = self.value(body);
                if let Some(expected) = self.frames[child].result {
                    let resolved = self.resolve_result_context(expected, result)?;
                    if resolved != expected {
                        self.frames[child].result = Some(resolved);
                        self.frames[child].error =
                            self.result_parts(resolved)?.map(|(_, _, error)| error);
                    }
                }
                if self.frames[child].result.is_none() {
                    f.result = result;
                    self.frames[child].result = Some(result);
                } else {
                    let declared = f.result;
                    f.result = self.frames[child].result.unwrap();
                    // The frame result names the body's own parameter
                    // scope; the callable type keeps its binder-relative
                    // `@captures` selection.
                    if let Some(selection @ ErasedCaptures::FromArguments(_)) = self
                        .types
                        .get(declared)
                        .and_then(|row| row.erased_captures.clone())
                    {
                        let mut row = self.types.get(f.result).unwrap().clone();
                        row.erased_captures = Some(selection);
                        f.result = self.types.intern(row);
                    }
                }
                f.error = result_error(self.types, f.result, &self.input.signature.metadata.roles);
                if !self.bottom(result)
                    && !self.invalid(result)
                    && self.frames[child]
                        .result
                        .is_some_and(|r| !self.compatible(result, r))
                {
                    self.type_mismatch(
                        const { RuleId::of("LANG-7") },
                        self.span(id),
                        "closure exits have different result types",
                        result,
                        self.frames[child].result.unwrap(),
                    );
                }
                let mut ty = self.ty(TypeKind::Function(f));
                // Body results name the literal's lexical parameter scope;
                // publish them under the callable's own binder.
                ty = abstract_parameter_scope(self.types, ty, self.site(NameSite::Expression(id)))
                    .expect("checked literal result binder");
                if expected.is_none() {
                    let mut row = self.types.get(ty).unwrap().clone();
                    row.modes.relation = Relation::environment();
                    ty = self.types.intern(row);
                }
                self.body.closures.push(ClosureInfo {
                    context: hirsel_types::CaptureContext::Scoped,
                    result_evidence: Some(result),
                    expression: id,
                    captures: vec![],
                    ty,
                });
                self.put(id, ty, expected, UseKind::Borrow)?;
            }
            After::LocalFunction(id, parent, _child, f) => {
                if let Some((_, local)) = self.local_function(id) {
                    let local = *local;
                    let ty = self.ty(TypeKind::Function(f));
                    self.frames[parent].locals.insert(local, ty);
                    self.body.bindings.push(BindingDecision {
                        owner: self.frames[parent].owner.clone(),
                        binding: ResolvedName::Local(local),
                        site: NameSite::Statement(id),
                        ty,
                        usage: UseKind::Borrow,
                        mutable: false,
                    });
                }
            }
        }
        Ok(())
    }
    fn binding_use(
        &mut self,
        expression: ast::ExprId,
        ty: TypeId,
        frame: usize,
    ) -> Result<UseKind, QueryError> {
        if self.bound(ty, ast::Bound::Plain)? {
            return Ok(UseKind::Copy);
        }
        if let ast::ExprKind::Unary { op, .. } =
            self.input.syntax.tree.expressions[expression.0 as usize].kind
        {
            return Ok(match op {
                ast::UnaryOp::Copy => UseKind::Copy,
                ast::UnaryOp::Mut => UseKind::Borrow,
                _ => UseKind::Take,
            });
        }
        let parameter = self.input_name(expression).map(|(_, r)| r);
        if let Some(r @ ResolvedName::Parameter(_)) = parameter {
            if let Some(t) = self.lookup_binding(expression, r, frame) {
                if self
                    .body
                    .bindings
                    .iter()
                    .any(|b| b.binding == *r && b.ty == t && b.usage == UseKind::Take)
                {
                    return Ok(UseKind::Take);
                }
            }
            return Ok(UseKind::Borrow);
        }
        Ok(UseKind::Take)
    }
    fn readonly_byte_projection(&self, mut expression: ast::ExprId) -> bool {
        loop {
            match self.input.syntax.tree.expressions[expression.0 as usize].kind {
                ast::ExprKind::Index { base, .. } => {
                    if matches!(
                        self.kind(self.value(base)),
                        TypeKind::Primitive(Primitive::Bytes)
                    ) {
                        return true;
                    }
                    expression = base;
                }
                ast::ExprKind::Field { base, .. } => expression = base,
                _ => return false,
            }
        }
    }
    fn mutable_place(
        &self,
        mut expression: ast::ExprId,
        _frame: usize,
        simple: bool,
    ) -> Option<RuleId> {
        let whole = expression;
        loop {
            match self.input.syntax.tree.expressions[expression.0 as usize].kind {
                ast::ExprKind::Index { base, .. }
                    if matches!(
                        self.kind(self.value(base)),
                        TypeKind::Primitive(Primitive::Bytes)
                    ) =>
                {
                    return Some(const { RuleId::of("LANG-5") });
                }
                ast::ExprKind::Field { base, .. } | ast::ExprKind::Index { base, .. } => {
                    expression = base
                }
                ast::ExprKind::Path(_) => {
                    let r = self.input_name(expression).map(|(_, r)| r);
                    if let Some(ResolvedName::Parameter(_)) = r {
                        return None;
                    } // OWN-1/3 belong to I.
                    let Some(ResolvedName::Local(l)) = r else {
                        return Some(const { RuleId::of("LANG-5") });
                    };
                    let b = self
                        .body
                        .bindings
                        .iter()
                        .find(|b| b.binding == ResolvedName::Local(*l));
                    // Ownership distinguishes whole view rebinding from
                    // referent mutation. Projected/compound writes remain read-only.
                    if self.mut_aliases.contains(l)
                        && !(simple && expression == whole && b.is_some_and(|b| b.mutable))
                    {
                        return Some(const { RuleId::of("LANG-15") });
                    }
                    return if b.is_some_and(|b| b.mutable) {
                        None
                    } else {
                        Some(const { RuleId::of("LANG-5") })
                    };
                }
                _ => return Some(const { RuleId::of("LANG-5") }),
            }
        }
    }
}
fn passing(p: ast::PassingMode) -> UseKind {
    match p {
        ast::PassingMode::Borrow => UseKind::Borrow,
        ast::PassingMode::Take => UseKind::Take,
        ast::PassingMode::Mut => UseKind::Mut,
    }
}
