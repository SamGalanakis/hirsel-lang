use super::*;

/// How an evaluation leaves a binding that it starts owning (MEM-5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ownership {
    /// Moved into a consumer on every path, with nothing that may fail
    /// while it is still owned.
    Moved,
    /// Its value is the evaluation's own value on every path, so whatever
    /// receives that value owns it.
    Yielded,
    /// Still owned on every path; nothing that may fail or exit ran.
    Kept,
    /// Some path may fail, exit or end with the binding still owned.
    Exposed,
}
impl Ownership {
    /// Two paths that rejoin.
    fn join(self, other: Self) -> Self {
        if self == other { self } else { Self::Exposed }
    }
    /// Where the evaluation's value is received by an owner.
    fn received(self) -> Self {
        match self {
            Self::Yielded => Self::Moved,
            other => other,
        }
    }
    /// Where the evaluation's value is destroyed.
    fn discarded(self) -> Self {
        match self {
            Self::Yielded => Self::Exposed,
            other => other,
        }
    }
}

#[derive(Clone)]
struct AllocationReason {
    span: hirsel_span::Span,
    kind: AllocationReasonKind,
}
#[derive(Clone)]
enum AllocationReasonKind {
    Call,
    Copy,
    Construction,
    Mutation,
    Cleanup {
        summary: DropAllocation,
        notes: Vec<String>,
    },
    Fields,
}
impl AllocationReason {
    fn diagnostic(self) -> Diagnostic {
        let message = match &self.kind {
            AllocationReasonKind::Call => "every call under noalloc requires a noalloc signature",
            AllocationReasonKind::Copy => "noalloc forbids an unproved deep copy or detach",
            AllocationReasonKind::Construction => {
                "noalloc forbids constructing a block whose destination may be the counted heap"
            }
            AllocationReasonKind::Mutation => "noalloc forbids a possible copy-on-write detach",
            AllocationReasonKind::Cleanup { .. } => {
                "an owned value under noalloc must satisfy noalloc_drop"
            }
            AllocationReasonKind::Fields => {
                "noalloc destruction requires nonallocating owned-field cleanup"
            }
        };
        let mut diagnostic = Diagnostic::error(const { RuleId::of("MEM-5") }, self.span, message);
        match self.kind {
            AllocationReasonKind::Cleanup { summary, notes } => {
                diagnostic
                    .notes
                    .push(format!("declared destruction summary: {summary:?}"));
                diagnostic.notes.extend(notes);
            }
            AllocationReasonKind::Fields => diagnostic.notes.push(
                "owned fields must satisfy noalloc_drop independently of the finalizer body".into(),
            ),
            _ => (),
        }
        diagnostic
    }
}

#[derive(Clone)]
struct EffectResult {
    allocation: Allocation,
    reasons: Vec<AllocationReason>,
}
impl Default for EffectResult {
    fn default() -> Self {
        Self {
            allocation: Allocation::NoAlloc,
            reasons: vec![],
        }
    }
}
impl EffectResult {
    fn add(&mut self, span: hirsel_span::Span, kind: AllocationReasonKind) {
        self.allocation = Allocation::MayAllocate;
        self.reasons.push(AllocationReason { span, kind });
    }
    fn join(&mut self, other: Self) {
        if other.allocation == Allocation::MayAllocate {
            self.allocation = Allocation::MayAllocate;
        }
        self.reasons.extend(other.reasons);
    }
}
#[derive(Clone, Default)]
struct BodyProof {
    execution: EffectResult,
    // Declared noalloc frames impose obligations even when the enclosing frame
    // permits allocation. Their bodies execute separately from construction.
    nested: EffectResult,
}
#[derive(Clone)]
struct FrameProof {
    noalloc: bool,
    cleanup: EffectResult,
    contracts: EffectResult,
    body: BodyProof,
}
impl FrameProof {
    fn required(self) -> EffectResult {
        let mut result = EffectResult::default();
        if self.noalloc {
            result.join(self.cleanup);
            result.join(self.contracts);
            result.join(self.body.execution);
        }
        result.join(self.body.nested);
        result
    }
}
// This cache belongs to one immutable checked body and effect-input snapshot.
// Every local frame uses escaping=true for its result; TypeIds and destinations
// must not be reused across audit invocations or caller specializations.
type LocalProofs = BTreeMap<NameSite, FrameProof>;

impl<'a> Audit<'a, '_> {
    fn copy_has_blocks(&mut self, root: TypeId) -> Result<bool, QueryError> {
        // Follow owned components, not referenced Shared payloads. Unknown
        // generic or opaque shapes need a written Plain proof.
        let mut todo = vec![(root, None)];
        let mut active = BTreeSet::new();
        while let Some((ty, leaving)) = todo.pop() {
            if let Some(path) = leaving {
                active.remove(&path);
                continue;
            }
            let children = match self.types.get(ty).unwrap().kind.clone() {
                TypeKind::Primitive(p) if !matches!(p, Primitive::String | Primitive::Bytes) => {
                    vec![]
                }
                TypeKind::Builtin {
                    kind: BuiltinType::Shared,
                    ..
                } => vec![],
                TypeKind::Tuple(fields) => fields.to_vec(),
                TypeKind::Record(fields) => fields.iter().map(|f| f.ty).collect(),
                TypeKind::Variant(variants) => variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|(_, ty)| *ty))
                    .collect(),
                TypeKind::Builtin {
                    kind: BuiltinType::Inline | BuiltinType::Simd,
                    arguments,
                } => arguments
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(ty) => Some(*ty),
                        _ => None,
                    })
                    .collect(),
                TypeKind::Parameter(parameter)
                    if self
                        .input
                        .checked
                        .resolution
                        .generic_declarations
                        .bounds(parameter)
                        .contains(&Bound::Plain) =>
                {
                    vec![]
                }
                TypeKind::Named { definition, .. } => {
                    if !active.insert(definition.clone()) {
                        return Ok(true);
                    }
                    let (representation, _) = self.representation(ty)?;
                    if representation == ty {
                        let signature = self.snapshot(&definition)?;
                        if !matches!(&signature.kind, SignatureKind::Type(d) if d.bounds.contains(&Bound::Plain))
                        {
                            return Ok(true);
                        }
                        active.remove(&definition);
                        vec![]
                    } else {
                        todo.push((ty, Some(definition)));
                        vec![representation]
                    }
                }
                _ => return Ok(true),
            };
            todo.extend(children.into_iter().map(|child| (child, None)));
        }
        Ok(false)
    }
    pub(super) fn allocations(&mut self) -> Result<(), QueryError> {
        let item = &self.input.tree.items[self.input.item.0 as usize];
        let owner = NameSite::Item(self.input.item);
        let mut proofs = LocalProofs::new();
        let mut result = EffectResult::default();
        match (&item.kind, &self.input.signature.kind) {
            (
                ast::ItemKind::Function {
                    signature, body, ..
                },
                SignatureKind::Function(f),
            ) => {
                if f.allocation == Allocation::NoAlloc {
                    result.join(self.cleanup(&owner, *body)?);
                    for contract in &signature.contracts {
                        result.join(self.contract_allocations(contract.expression)?);
                    }
                }
                if let Some(root) = body {
                    let store = &self.input.signature.types;
                    let arena = self
                        .input
                        .signature
                        .metadata
                        .roles
                        .heap
                        .as_ref()
                        .is_some_and(|heap| {
                            store
                                .get(f.result)
                                .unwrap()
                                .modes
                                .relation
                                .scoped(|o| arena_parameter(store, f, o, 0, &heap.arena))
                        });
                    let body = self.walk_allocations(
                        owner,
                        *root,
                        f.allocation == Allocation::NoAlloc,
                        !arena,
                        &mut proofs,
                    )?;
                    if f.allocation == Allocation::NoAlloc {
                        result.join(body.execution);
                    }
                    result.join(body.nested);
                }
            }
            (
                ast::ItemKind::Type {
                    drop: Some(drop), ..
                },
                SignatureKind::Type(d),
            ) => {
                if d.destructor_allocation == Allocation::NoAlloc {
                    if let Some(repr) = d.representation
                        && self
                            .input
                            .signature
                            .types
                            .get(repr)
                            .unwrap()
                            .drop_allocation
                            != DropAllocation::NoAlloc
                    {
                        result.add(
                            self.input.tree.expressions[drop.body.0 as usize]
                                .origin
                                .span,
                            AllocationReasonKind::Fields,
                        );
                    }
                    result.join(self.cleanup(&owner, Some(drop.body))?);
                }
                let body = self.walk_allocations(
                    owner,
                    drop.body,
                    d.destructor_allocation == Allocation::NoAlloc,
                    false,
                    &mut proofs,
                )?;
                if d.destructor_allocation == Allocation::NoAlloc {
                    result.join(body.execution);
                }
                result.join(body.nested);
            }
            _ => (),
        }
        self.diagnostics
            .extend(result.reasons.into_iter().map(AllocationReason::diagnostic));
        Ok(())
    }
    fn transferred_name(&self, expression: ExprId) -> Option<ResolvedName> {
        let place = self.source_place(expression)?;
        let takes = place.access == SourceAccess::Take
            || (place.access == SourceAccess::Read
                && self
                    .input
                    .checked
                    .expressions
                    .iter()
                    .any(|r| r.expression == expression && r.usage == UseKind::Take));
        if !takes || !place.projections.is_empty() {
            return None;
        }
        match &place.root {
            SourceRoot::Binding { binding, .. } => Some(binding.clone()),
            _ => None,
        }
    }
    fn function_syntax(&self, owner: &NameSite) -> Option<&'a ast::FunctionSyntax> {
        match owner {
            NameSite::Item(i) => match &self.input.tree.items[i.0 as usize].kind {
                ast::ItemKind::Function { signature, .. } => Some(signature),
                _ => None,
            },
            NameSite::Statement(s) => match &self.input.tree.statements[s.0 as usize].kind {
                ast::StmtKind::Function { signature, .. } => Some(signature),
                _ => None,
            },
            NameSite::Expression(e) => match &self.input.tree.expressions[e.0 as usize].kind {
                ast::ExprKind::Closure { signature, .. } => Some(signature),
                _ => None,
            },
            _ => None,
        }
    }
    fn drop_allowed(&self, ty: TypeId, _owner: &NameSite) -> bool {
        self.summary_allowed(&self.types.get(ty).unwrap().drop_allocation)
    }
    fn summary_allowed(&self, summary: &DropAllocation) -> bool {
        match summary {
            DropAllocation::NoAlloc => true,
            DropAllocation::MayAllocate => false,
            DropAllocation::DependsOn(parameters) => parameters.iter().all(|p| {
                let bounds = self
                    .input
                    .checked
                    .resolution
                    .generic_declarations
                    .bounds(*p);
                bounds.contains(&Bound::NoallocDrop) || bounds.contains(&Bound::Plain)
            }),
        }
    }
    fn destruction_notes(&mut self, root: TypeId) -> Result<Vec<String>, QueryError> {
        let mut notes = Vec::new();
        let mut todo = vec![root];
        let mut seen = BTreeSet::new();
        let mut definitions = BTreeSet::new();
        while let Some(ty) = todo.pop() {
            if !seen.insert(ty) {
                continue;
            }
            match self.types.get(ty).unwrap().kind.clone() {
                TypeKind::Named { definition, .. } => {
                    if !definitions.insert(definition.clone()) {
                        continue;
                    }
                    let signature = self.snapshot(&definition)?;
                    if matches!(&signature.kind, SignatureKind::Type(d) if d.resource && d.destructor_allocation != Allocation::NoAlloc)
                    {
                        notes.push(format!("destructor {definition} permits allocation"));
                    }
                    let (repr, _) = self.representation(ty)?;
                    if repr != ty {
                        todo.push(repr);
                    }
                }
                TypeKind::Tuple(ts) => todo.extend(ts),
                TypeKind::Record(fs) => todo.extend(fs.iter().map(|f| f.ty)),
                TypeKind::Variant(vs) => {
                    todo.extend(vs.iter().flat_map(|v| v.fields.iter().map(|(_, t)| *t)))
                }
                TypeKind::Box(t) => todo.push(t),
                TypeKind::Builtin { arguments, .. } => {
                    todo.extend(arguments.iter().filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(*t),
                        _ => None,
                    }))
                }
                _ => (),
            }
        }
        notes.sort();
        notes.dedup();
        Ok(notes)
    }
    fn cleanup(
        &mut self,
        owner: &NameSite,
        root: Option<ExprId>,
    ) -> Result<EffectResult, QueryError> {
        let mut result = EffectResult::default();
        let can_fail_contract = self.function_syntax(owner).is_some_and(|s| {
            s.contracts.iter().any(|c| {
                !matches!(
                    self.input.tree.expressions[c.expression.0 as usize].kind,
                    ast::ExprKind::Literal(ast::Literal::Bool(true))
                )
            })
        });
        let lets = root.map(|root| self.let_patterns(root)).unwrap_or_default();
        for binding in &self.input.checked.bindings {
            if binding.owner != *owner
                || binding.usage == UseKind::Borrow
                || binding.usage == UseKind::Mut
            {
                continue;
            }
            if let ResolvedName::Parameter(p) = binding.binding {
                let syntax = self.function_syntax(owner);
                if syntax
                    .and_then(|s| s.parameters.get(p.0 as usize))
                    .is_some_and(|p| p.passing != PassingMode::Take)
                {
                    continue;
                }
            }
            let Some(ty) = self.types.get(binding.ty).cloned() else {
                continue;
            };
            if self.drop_allowed(binding.ty, owner) {
                continue;
            }
            // MEM-5: "A value transferred on every exit without any possible
            // intervening failure creates no cleanup obligation." A taken
            // parameter is owned from entry; a let binding from its statement
            // to the end of its block.
            if !can_fail_contract {
                let moved = match (&binding.binding, binding.site, root) {
                    (ResolvedName::Parameter(_), _, Some(root)) => {
                        self.ownership(&binding.binding, root).received()
                    }
                    (ResolvedName::Local(_), NameSite::Pattern(pattern), _) => {
                        match lets.get(&pattern) {
                            Some(&(block, index)) => {
                                let ast::ExprKind::Block { statements, tail } =
                                    self.input.tree.expressions[block.0 as usize].kind.clone()
                                else {
                                    unreachable!()
                                };
                                let rest =
                                    self.rest(&binding.binding, &statements[index + 1..], tail);
                                // The frame's own block yields its result;
                                // where an inner block's value goes is not
                                // followed.
                                if root == Some(block) {
                                    rest.received()
                                } else {
                                    rest.discarded()
                                }
                            }
                            None => Ownership::Exposed,
                        }
                    }
                    _ => Ownership::Exposed,
                };
                if moved == Ownership::Moved {
                    continue;
                }
            }
            // Only taken parameters own their entry values. Plain borrowed parameters
            // can be represented as Copy, and their cleanup is already NoAlloc.
            let parameter_type = if let ResolvedName::Parameter(p) = binding.binding {
                let syntax = self.function_syntax(owner);
                syntax
                    .and_then(|s| s.parameters.get(p.0 as usize))
                    .map(|p| self.input.tree.types[p.ty.0 as usize].origin.span)
            } else {
                None
            };
            let span = parameter_type.unwrap_or_else(|| match binding.site {
                NameSite::Pattern(p) => self.input.tree.patterns[p.0 as usize].origin.span,
                NameSite::Type(t) => self.input.tree.types[t.0 as usize].origin.span,
                NameSite::Item(i) => self.input.tree.items[i.0 as usize].name_span,
                NameSite::Expression(e) => self.input.tree.expressions[e.0 as usize].origin.span,
                NameSite::Statement(s) => self.input.tree.statements[s.0 as usize].origin.span,
            });
            let summary = ty.drop_allocation.clone();
            result.add(
                span,
                AllocationReasonKind::Cleanup {
                    summary,
                    notes: self.destruction_notes(binding.ty)?,
                },
            );
        }
        if let Some(root) = root {
            result.join(self.temporaries(root, can_fail_contract)?);
        }
        Ok(result)
    }
    /// Each let binding's pattern, with the block and statement that bind
    /// it. Nested frames are walked too: their bindings have other owners.
    fn let_patterns(&self, root: ExprId) -> BTreeMap<ast::PatternId, (ExprId, usize)> {
        let mut lets = BTreeMap::new();
        let mut todo = vec![root];
        while let Some(e) = todo.pop() {
            let kind = &self.input.tree.expressions[e.0 as usize].kind;
            match kind {
                ast::ExprKind::Block { statements, .. } => {
                    for (index, statement) in statements.iter().enumerate() {
                        match &self.input.tree.statements[statement.0 as usize].kind {
                            ast::StmtKind::Let { pattern, value, .. } => {
                                let mut patterns = vec![*pattern];
                                while let Some(p) = patterns.pop() {
                                    lets.insert(p, (e, index));
                                    match &self.input.tree.patterns[p.0 as usize].kind {
                                        ast::PatternKind::Tuple(parts) => patterns.extend(parts),
                                        ast::PatternKind::Constructor { fields, .. } => {
                                            patterns.extend(fields.iter().map(|f| f.pattern))
                                        }
                                        _ => (),
                                    }
                                }
                                todo.push(*value);
                            }
                            ast::StmtKind::Expr(value) => todo.push(*value),
                            ast::StmtKind::Assign { place, value, .. } => {
                                todo.extend([*place, *value])
                            }
                            ast::StmtKind::While {
                                condition, body, ..
                            } => todo.extend([*condition, *body]),
                            ast::StmtKind::For { source, body, .. } => {
                                todo.extend([*source, *body])
                            }
                            ast::StmtKind::Function { body, .. } => todo.push(*body),
                        }
                    }
                }
                ast::ExprKind::Closure { body, .. } => todo.push(*body),
                _ => (),
            }
            todo.extend(children(kind));
        }
        lets
    }
    /// How evaluating `e` leaves `name`, owned before it starts (MEM-5).
    fn ownership(&mut self, name: &ResolvedName, e: ExprId) -> Ownership {
        if self.transferred_name(e).as_ref() == Some(name) {
            return Ownership::Yielded;
        }
        match self.input.tree.expressions[e.0 as usize].kind.clone() {
            ast::ExprKind::Block { statements, tail } => self.rest(name, &statements, tail),
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => match self.ownership(name, condition).discarded() {
                Ownership::Kept => {
                    let then = self.ownership(name, then_branch);
                    let other = else_branch.map_or(Ownership::Kept, |e| self.ownership(name, e));
                    then.join(other)
                }
                other => other,
            },
            // A scrutinee that yields the binding is matched in place; its
            // payload bindings keep their own obligations.
            ast::ExprKind::Match { value, arms } => match self.ownership(name, value).discarded() {
                Ownership::Kept => {
                    let mut joined = None;
                    for arm in arms {
                        let guard = arm.guard.map_or(Ownership::Kept, |guard| {
                            self.ownership(name, guard).discarded()
                        });
                        let arm = match guard {
                            Ownership::Kept => self.ownership(name, arm.body),
                            other => other,
                        };
                        joined = Some(joined.map_or(arm, |joined: Ownership| joined.join(arm)));
                    }
                    joined.unwrap_or(Ownership::Kept)
                }
                other => other,
            },
            ast::ExprKind::Call { callee, arguments } => match self.ownership(name, callee) {
                Ownership::Kept => {
                    // A constructor owns its fields; a call owns its `take`
                    // arguments from entry and only lends the others.
                    let constructor = self.infallible_constructor(callee);
                    let operands = arguments
                        .into_iter()
                        .map(|a| {
                            let owned = constructor
                                || self.call_parameters.get(&(e, a.value)).is_some_and(
                                    |(_, parameter)| parameter.passing == PassingMode::Take,
                                );
                            (a.value, owned)
                        })
                        .collect();
                    self.operands(name, operands, !constructor)
                }
                other => other.discarded(),
            },
            ast::ExprKind::Tuple(parts) | ast::ExprKind::Sequence(parts) => {
                self.operands(name, parts.into_iter().map(|p| (p, true)).collect(), false)
            }
            ast::ExprKind::Return(Some(value)) => match self.ownership(name, value).received() {
                Ownership::Moved => Ownership::Moved,
                _ => Ownership::Exposed,
            },
            _ if self.may_fail(e) => Ownership::Exposed,
            _ => Ownership::Kept,
        }
    }
    /// The rest of a block: its remaining statements, then its tail. What
    /// is still owned at its end is destroyed there.
    fn rest(
        &mut self,
        name: &ResolvedName,
        statements: &[ast::StmtId],
        tail: Option<ExprId>,
    ) -> Ownership {
        for statement in statements {
            let step = match &self.input.tree.statements[statement.0 as usize].kind {
                ast::StmtKind::Expr(value) => self.ownership(name, *value).discarded(),
                // A pattern that binds every part owns what it receives.
                ast::StmtKind::Let { pattern, value, .. } => {
                    let step = self.ownership(name, *value);
                    if self.discards(*pattern) {
                        step.discarded()
                    } else {
                        step.received()
                    }
                }
                ast::StmtKind::Function { .. } => Ownership::Kept,
                // A replacement destroys what it overwrites, and a loop may
                // run any number of times: neither is followed here.
                ast::StmtKind::Assign { .. }
                | ast::StmtKind::While { .. }
                | ast::StmtKind::For { .. } => Ownership::Exposed,
            };
            if step != Ownership::Kept {
                return step;
            }
        }
        tail.map_or(Ownership::Kept, |tail| self.ownership(name, tail))
    }
    /// Operands evaluated in order, each with whether their consumer owns
    /// it. Once one yields `name` to the consumer, every later operand must
    /// be unable to fail, and so must the consumer while none has.
    fn operands(
        &mut self,
        name: &ResolvedName,
        operands: Vec<(ExprId, bool)>,
        consumer_may_fail: bool,
    ) -> Ownership {
        let mut taken = false;
        for (operand, owned) in operands {
            if taken {
                if self.may_fail(operand) {
                    return Ownership::Exposed;
                }
                continue;
            }
            match self.ownership(name, operand) {
                Ownership::Kept => (),
                Ownership::Yielded if owned => taken = true,
                Ownership::Yielded => return Ownership::Exposed,
                other => return other,
            }
        }
        if taken {
            Ownership::Moved
        } else if consumer_may_fail {
            Ownership::Exposed
        } else {
            Ownership::Kept
        }
    }
    /// A constructor of a type that checks no fact and is not a resource.
    fn infallible_constructor(&mut self, callee: ExprId) -> bool {
        match self.source_name(callee).map(|(_, n)| n.clone()) {
            Some(ResolvedName::Definition(path)) => self.snapshot(&path).is_ok_and(
                |s| matches!(&s.kind, SignatureKind::Type(d) if !d.resource && d.fact.is_none()),
            ),
            _ => false,
        }
    }
    /// MEM-5: a temporary is destroyed in its frame when its value is
    /// discarded (a statement, a wildcard, a lent argument or projected
    /// base) or when an evaluation that may fail follows it before its
    /// consumer owns it ("consuming it into another call does not remove
    /// cleanup obligations on earlier failure paths"). Under `@noalloc` its
    /// cleanup must not allocate, as a binding's must not. Nested functions
    /// and literals are their own frames.
    fn temporaries(&mut self, root: ExprId, can_fail: bool) -> Result<EffectResult, QueryError> {
        let mut result = EffectResult::default();
        let mut todo = vec![(root, can_fail)];
        while let Some((e, destroyed)) = todo.pop() {
            let kind = self.input.tree.expressions[e.0 as usize].kind.clone();
            // A read of a binding's storage or of a constant destroys nothing.
            let storage = self.source_place(e).is_some_and(|place| match place.root {
                SourceRoot::Binding { .. } => matches!(
                    place.access,
                    SourceAccess::Read | SourceAccess::Take | SourceAccess::Mut
                ),
                SourceRoot::Definition(_) => true,
                SourceRoot::Value(_) => false,
            });
            let transparent = matches!(
                kind,
                ast::ExprKind::Block { .. }
                    | ast::ExprKind::If { .. }
                    | ast::ExprKind::Match { .. }
            );
            if destroyed && !storage && !transparent {
                if let Some(ty) = self.expression_type(e) {
                    // A literal's environment destroys what it owns; a
                    // borrowed capture destroys nothing (CLO-1).
                    let summary = match kind {
                        ast::ExprKind::Closure { .. } => self
                            .input
                            .checked
                            .closures
                            .iter()
                            .find(|closure| closure.expression == e)
                            .map_or(DropAllocation::MayAllocate, |closure| {
                                closure
                                    .captures
                                    .iter()
                                    .filter(|c| matches!(c.usage, UseKind::Copy | UseKind::Take))
                                    .fold(DropAllocation::NoAlloc, |summary, capture| {
                                        summary.join(
                                            self.types
                                                .get(capture.ty)
                                                .unwrap()
                                                .drop_allocation
                                                .clone(),
                                        )
                                    })
                            }),
                        _ => self.types.get(ty).unwrap().drop_allocation.clone(),
                    };
                    if !self.summary_allowed(&summary) {
                        result.add(
                            self.input.tree.expressions[e.0 as usize].origin.span,
                            AllocationReasonKind::Cleanup {
                                summary,
                                notes: self.destruction_notes(ty)?,
                            },
                        );
                    }
                }
            }
            match kind {
                ast::ExprKind::Block { statements, tail } => {
                    if let Some(tail) = tail {
                        todo.push((tail, destroyed));
                    }
                    for statement in statements {
                        match &self.input.tree.statements[statement.0 as usize].kind {
                            ast::StmtKind::Expr(value) => todo.push((*value, true)),
                            ast::StmtKind::Let { pattern, value, .. } => {
                                todo.push((*value, self.discards(*pattern)))
                            }
                            ast::StmtKind::Assign { place, value, .. } => {
                                todo.push((*place, false));
                                todo.push((*value, false));
                            }
                            ast::StmtKind::While {
                                condition, body, ..
                            } => {
                                todo.push((*condition, true));
                                todo.push((*body, true));
                            }
                            // A loop consumes its source through a cursor
                            // protocol, which declares its own cleanup.
                            ast::StmtKind::For { source, body, .. } => {
                                todo.push((*source, false));
                                todo.push((*body, true));
                            }
                            ast::StmtKind::Function { .. } => (),
                        }
                    }
                }
                ast::ExprKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    todo.push((condition, true));
                    todo.push((then_branch, destroyed));
                    if let Some(other) = else_branch {
                        todo.push((other, destroyed));
                    }
                }
                ast::ExprKind::Match { value, arms } => {
                    todo.push((value, true));
                    for arm in arms {
                        if let Some(guard) = arm.guard {
                            todo.push((guard, true));
                        }
                        todo.push((arm.body, destroyed));
                    }
                }
                ast::ExprKind::Call { callee, arguments } => {
                    todo.push((callee, false));
                    let lent: Vec<_> = arguments
                        .iter()
                        .map(|argument| {
                            let lent = self.call_parameters.get(&(e, argument.value)).is_some_and(
                                |(_, parameter)| {
                                    matches!(
                                        parameter.passing,
                                        PassingMode::Borrow | PassingMode::Mut
                                    )
                                },
                            );
                            (argument.value, lent)
                        })
                        .collect();
                    self.ordered(&mut todo, lent);
                }
                ast::ExprKind::Tuple(parts) | ast::ExprKind::Sequence(parts) => {
                    self.ordered(&mut todo, parts.into_iter().map(|p| (p, false)).collect());
                }
                ast::ExprKind::Binary { left, right, .. } => {
                    self.ordered(&mut todo, vec![(left, true), (right, true)]);
                }
                ast::ExprKind::Field { base, .. } if !storage => todo.push((base, true)),
                ast::ExprKind::Index { base, index } => {
                    if !storage {
                        todo.push((base, true));
                    }
                    todo.push((index, true));
                }
                ast::ExprKind::Unary { op, value } => match op {
                    ast::UnaryOp::Take | ast::UnaryOp::Copy | ast::UnaryOp::Unique => (),
                    ast::UnaryOp::Mut => todo.push((value, false)),
                    ast::UnaryOp::Box => todo.push((value, destroyed)),
                    _ => todo.push((value, true)),
                },
                ast::ExprKind::Try { value, .. } | ast::ExprKind::Return(Some(value)) => {
                    todo.push((value, false))
                }
                _ => (),
            }
        }
        Ok(result)
    }
    /// Operands evaluated in order: each is destroyed if it is lent or a
    /// later one may fail before the consumer owns it.
    fn ordered(&mut self, todo: &mut Vec<(ExprId, bool)>, operands: Vec<(ExprId, bool)>) {
        let mut later = false;
        for (operand, lent) in operands.into_iter().rev() {
            todo.push((operand, lent || later));
            later = later || self.may_fail(operand);
        }
    }
    /// A pattern that does not bind every part of its value destroys the rest.
    fn discards(&self, pattern: ast::PatternId) -> bool {
        let mut todo = vec![pattern];
        while let Some(p) = todo.pop() {
            match &self.input.tree.patterns[p.0 as usize].kind {
                ast::PatternKind::Wildcard => return true,
                ast::PatternKind::Tuple(parts) => todo.extend(parts),
                ast::PatternKind::Constructor { fields, rest, .. } => {
                    if *rest {
                        return true;
                    }
                    todo.extend(fields.iter().map(|f| f.pattern));
                }
                _ => (),
            }
        }
        false
    }
    /// Whether evaluating an expression may fail. Only reads, literals,
    /// closures and constructions of types that check no fact never fail.
    fn may_fail(&mut self, e: ExprId) -> bool {
        match self.input.tree.expressions[e.0 as usize].kind.clone() {
            ast::ExprKind::Literal(_) | ast::ExprKind::Path(_) | ast::ExprKind::Closure { .. } => {
                false
            }
            ast::ExprKind::Tuple(parts) | ast::ExprKind::Sequence(parts) => {
                parts.into_iter().any(|p| self.may_fail(p))
            }
            ast::ExprKind::Field { base, .. } => self.may_fail(base),
            ast::ExprKind::Unary {
                op:
                    ast::UnaryOp::Take
                    | ast::UnaryOp::Copy
                    | ast::UnaryOp::Mut
                    | ast::UnaryOp::Box
                    | ast::UnaryOp::Not,
                value,
            } => self.may_fail(value),
            ast::ExprKind::Call { callee, arguments } => {
                !self.infallible_constructor(callee)
                    || arguments.into_iter().any(|a| self.may_fail(a.value))
            }
            _ => true,
        }
    }
    fn contract_allocations(&mut self, root: ExprId) -> Result<EffectResult, QueryError> {
        let mut result = EffectResult::default();
        // Contracts belong to signature code. H/J currently check executable bodies;
        // resolve contract calls through declarations, never through implementations.
        let mut todo = vec![root];
        while let Some(e) = todo.pop() {
            let expression = &self.input.tree.expressions[e.0 as usize];
            if let ast::ExprKind::Call { callee, .. } = expression.kind {
                let resolved = self.source_name(callee).map(|(_, n)| n.clone());
                let allocation = match resolved {
                    Some(ResolvedName::Definition(d)) => {
                        let signature = self.snapshot(&d)?;
                        match &signature.kind {
                            SignatureKind::Function(f) => Some(f.allocation),
                            _ => None,
                        }
                    }
                    Some(name @ (ResolvedName::Parameter(_) | ResolvedName::Local(_))) => self
                        .source_binding(&name, None)
                        .and_then(|b| self.types.get(b.ty))
                        .and_then(|t| match &t.kind {
                            TypeKind::Function(f) => Some(f.allocation),
                            _ => None,
                        }),
                    _ => None,
                };
                if allocation == Some(Allocation::MayAllocate) {
                    result.add(expression.origin.span, AllocationReasonKind::Call);
                }
            }
            todo.extend(children(&expression.kind));
        }
        Ok(result)
    }
    fn local_frame(
        &mut self,
        owner: NameSite,
        body: ExprId,
        noalloc: bool,
        proofs: &mut LocalProofs,
    ) -> Result<FrameProof, QueryError> {
        if let Some(proof) = proofs.get(&owner) {
            return Ok(proof.clone());
        }
        let cleanup = if noalloc {
            self.cleanup(&owner, Some(body))?
        } else {
            EffectResult::default()
        };
        let mut contracts = EffectResult::default();
        if noalloc {
            for contract in &self.function_syntax(&owner).unwrap().contracts {
                contracts.join(self.contract_allocations(contract.expression)?);
            }
        }
        let body = self.walk_allocations(owner, body, noalloc, true, proofs)?;
        let proof = FrameProof {
            noalloc,
            cleanup,
            contracts,
            body,
        };
        proofs.insert(owner, proof.clone());
        Ok(proof)
    }
    fn closure_proof(
        &mut self,
        literal: ExprId,
        proofs: &mut LocalProofs,
    ) -> Result<FrameProof, QueryError> {
        let ast::ExprKind::Closure { body, .. } =
            self.input.tree.expressions[literal.0 as usize].kind
        else {
            unreachable!("local inference requires an immediate closure literal");
        };
        let noalloc = self.expression_type(literal).and_then(|ty| self.types.get(ty))
            .is_some_and(|row| matches!(&row.kind, TypeKind::Function(f) if f.allocation == Allocation::NoAlloc));
        self.local_frame(NameSite::Expression(literal), body, noalloc, proofs)
    }
    /// Implicit calls carry the same obligations as written calls. Plans
    /// select declarations; cursor types carry the producer's cleanup summary.
    fn loop_allocations(
        &mut self,
        statement: ast::StmtId,
        source: ExprId,
        result: &mut EffectResult,
    ) -> Result<(), QueryError> {
        use hirsel_check_protocol::body::{ForExecution, ForPlanKind, LoopLength};
        let Some(plan) = self
            .input
            .loops
            .iter()
            .find(|p| p.statement() == statement)
            .cloned()
        else {
            return Ok(());
        };
        let span = self.input.tree.expressions[source.0 as usize].origin.span;
        let (calls, cursor) = match plan.kind {
            ForPlanKind::Cursor { cursor } => (vec![cursor.open, cursor.next], Some(cursor.cursor)),
            ForPlanKind::Direct { execution, .. } => match execution {
                ForExecution::Advance { operation, .. } => {
                    // Taking an iterator transfers the existing state into
                    // loop storage; this protocol creates no separate cursor.
                    let taken = matches!(
                        self.input.tree.expressions[source.0 as usize].kind,
                        ast::ExprKind::Unary {
                            op: ast::UnaryOp::Take,
                            ..
                        }
                    );
                    (
                        vec![operation],
                        taken.then(|| self.expression_type(source)).flatten(),
                    )
                }
                ForExecution::Indexed {
                    length: LoopLength::Definition { definition, .. },
                    ..
                } => (vec![definition], None),
                _ => (vec![], None),
            },
        };
        for call in calls {
            if matches!(&self.snapshot(&call)?.kind, SignatureKind::Function(f) if f.allocation == Allocation::MayAllocate)
            {
                result.add(span, AllocationReasonKind::Call);
            }
        }
        if let Some(cursor) = cursor {
            if !self.drop_allowed(cursor, &NameSite::Statement(statement)) {
                result.add(
                    span,
                    AllocationReasonKind::Cleanup {
                        summary: self.types.get(cursor).unwrap().drop_allocation.clone(),
                        notes: self.destruction_notes(cursor)?,
                    },
                );
            }
        }
        Ok(())
    }

    fn walk_allocations(
        &mut self,
        frame: NameSite,
        root: ExprId,
        prohibited: bool,
        escaping: bool,
        proofs: &mut LocalProofs,
    ) -> Result<BodyProof, QueryError> {
        let mut result = BodyProof::default();
        let mut todo = vec![(root, prohibited, escaping)];
        let mut seen = BTreeSet::new();
        while let Some((e, prohibited, escaping)) = todo.pop() {
            if !seen.insert((e, prohibited, escaping)) {
                continue;
            }
            let expression = &self.input.tree.expressions[e.0 as usize];
            let Some(ty) = self.expression_type(e) else {
                continue;
            };
            let row = self.types.get(ty).unwrap().clone();
            let escaping = escaping && !self.fixed_size(ty, &mut BTreeSet::new())?;
            if matches!(row.kind, TypeKind::Error) {
                continue;
            }
            if prohibited {
                if self
                    .input
                    .call_allocations
                    .iter()
                    .any(|(id, a)| *id == e && *a == Allocation::MayAllocate)
                {
                    result
                        .execution
                        .add(expression.origin.span, AllocationReasonKind::Call);
                }
                if self.input.allocating_copies.contains(&e)
                    && !self.placed(&row.modes.relation)
                    && self.copy_has_blocks(ty)?
                {
                    result
                        .execution
                        .add(expression.origin.span, AllocationReasonKind::Copy);
                }
                let construction = matches!(
                    expression.kind,
                    ast::ExprKind::Literal(ast::Literal::String(_) | ast::Literal::Bytes(_))
                        | ast::ExprKind::Sequence(_)
                        | ast::ExprKind::Unary {
                            op: ast::UnaryOp::Box,
                            ..
                        }
                ) || matches!(expression.kind, ast::ExprKind::Closure { .. })
                    && row.modes.relation.is_global();
                // `in heap` is the empty relation in v22 and no longer a
                // separate placement: unplaced constructions count when they
                // escape this frame.
                let counted = !self.placed(&row.modes.relation) && escaping;
                if construction && counted {
                    result
                        .execution
                        .add(expression.origin.span, AllocationReasonKind::Construction);
                }
            }
            match &expression.kind {
                ast::ExprKind::Path(_) if escaping => {
                    let name = self.source_name(e).map(|(_, name)| name);
                    let scope = self
                        .source_scope(NameSite::Expression(e))
                        .map(|(_, scope)| scope);
                    // A captured binding was initialised in another frame.
                    if let Some(name @ ResolvedName::Local(_)) = name
                        && let Some(binding) = self.source_binding(name, scope.copied())
                        && binding.owner == frame
                        && let NameSite::Pattern(pattern) = binding.site
                    {
                        for statement in self.let_statements(pattern) {
                            if let ast::StmtKind::Let {
                                pattern: p, value, ..
                            } = statement.kind
                                && p == pattern
                            {
                                todo.push((value, prohibited, true));
                            }
                        }
                    }
                }
                ast::ExprKind::Block { statements, tail } => {
                    if let Some(tail) = tail {
                        todo.push((*tail, prohibited, escaping));
                    }
                    for s in statements {
                        let statement = &self.input.tree.statements[s.0 as usize];
                        match &statement.kind {
                            // A binding written `in heap` receives a global value
                            // that may outlive this frame (MEM-5, MEM-10).
                            ast::StmtKind::Let { value, ty, .. } => todo.push((
                                *value,
                                prohibited,
                                ty.is_some_and(|ty| self.written_heap(ty)),
                            )),
                            ast::StmtKind::Expr(value) => todo.push((*value, prohibited, false)),
                            ast::StmtKind::Assign { place, value, .. } => {
                                if prohibited && self.input.allocating_mutations.contains(place) {
                                    result
                                        .execution
                                        .add(statement.origin.span, AllocationReasonKind::Mutation);
                                }
                                todo.push((*place, prohibited, false));
                                todo.push((*value, prohibited, true));
                            }
                            ast::StmtKind::While {
                                condition, body, ..
                            } => {
                                todo.push((*condition, prohibited, false));
                                todo.push((*body, prohibited, false));
                            }
                            ast::StmtKind::For { source, body, .. } => {
                                if prohibited {
                                    self.loop_allocations(*s, *source, &mut result.execution)?;
                                }
                                todo.push((*source, prohibited, false));
                                todo.push((*body, prohibited, false));
                            }
                            ast::StmtKind::Function {
                                signature, body, ..
                            } => {
                                let noalloc = signature
                                    .attributes
                                    .iter()
                                    .any(|a| self.input.symbols.resolve(a.name) == Some("noalloc"));
                                result.nested.join(
                                    self.local_frame(
                                        NameSite::Statement(*s),
                                        *body,
                                        noalloc,
                                        proofs,
                                    )?
                                    .required(),
                                );
                            }
                        }
                    }
                }
                ast::ExprKind::Closure { .. } => {
                    result
                        .nested
                        .join(self.closure_proof(e, proofs)?.required());
                }
                ast::ExprKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    todo.push((*condition, prohibited, false));
                    todo.push((*then_branch, prohibited, escaping));
                    if let Some(other) = else_branch {
                        todo.push((*other, prohibited, escaping));
                    }
                }
                ast::ExprKind::Match { value, arms } => {
                    todo.push((*value, prohibited, false));
                    for arm in arms {
                        if let Some(guard) = arm.guard {
                            todo.push((guard, prohibited, false));
                        }
                        todo.push((arm.body, prohibited, escaping));
                    }
                }
                ast::ExprKind::Return(value) => {
                    if let Some(value) = value {
                        todo.push((*value, prohibited, true));
                    }
                }
                ast::ExprKind::Call { callee, arguments } => {
                    todo.push((*callee, prohibited, false));
                    // Calls have declared effects; constructors inherit the value destination.
                    let constructor = !self.input.call_allocations.iter().any(|(id, _)| *id == e);
                    let locality = self
                        .expression_type(*callee)
                        .and_then(|ty| self.types.get(ty))
                        .and_then(|row| match &row.kind {
                            TypeKind::Function(function) => self.types.get(function.result),
                            _ => None,
                        })
                        .map(|row| row.modes.relation.clone());
                    for arg in arguments {
                        let parameter = self.call_parameters.get(&(e, arg.value)).cloned();
                        let taken_global = parameter.as_ref().is_some_and(|(_, parameter)| {
                            parameter.passing == PassingMode::Take
                                && self
                                    .types
                                    .get(parameter.ty)
                                    .unwrap()
                                    .modes
                                    .relation
                                    .is_global()
                        });
                        let retained = escaping
                            && parameter
                                .as_ref()
                                .is_some_and(|(ordinal, _)| match &locality {
                                    Some(relation) if relation.has_environment() => true,
                                    Some(relation) => {
                                        relation.named_parameters().is_some_and(|ps| {
                                            ps.contains(&ParameterRef::bound(ParamId(*ordinal)))
                                        })
                                    }
                                    None => false,
                                });
                        // A global taken value can outlive this frame. A retained
                        // argument shares its caller's escaping destination even
                        // when the callee itself has a NoAlloc signature.
                        todo.push((
                            arg.value,
                            prohibited,
                            constructor && escaping || taken_global || retained,
                        ));
                    }
                }
                _ => {
                    for child in children(&expression.kind) {
                        todo.push((child, prohibited, escaping));
                    }
                }
            }
        }
        Ok(result)
    }
}

fn children(kind: &ast::ExprKind) -> Vec<ExprId> {
    match kind {
        ast::ExprKind::Tuple(es) | ast::ExprKind::Sequence(es) => es.clone(),
        ast::ExprKind::Call { callee, arguments } => std::iter::once(*callee)
            .chain(arguments.iter().map(|a| a.value))
            .collect(),
        ast::ExprKind::Field { base, .. } => vec![*base],
        ast::ExprKind::Index { base, index } => vec![*base, *index],
        ast::ExprKind::Unary { value, .. }
        | ast::ExprKind::Try { value, .. }
        | ast::ExprKind::Before(value)
        | ast::ExprKind::Comptime(value) => vec![*value],
        ast::ExprKind::Binary { left, right, .. } => vec![*left, *right],
        ast::ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => std::iter::once(*condition)
            .chain(std::iter::once(*then_branch))
            .chain(else_branch.iter().copied())
            .collect(),
        ast::ExprKind::Match { value, arms } => std::iter::once(*value)
            .chain(
                arms.iter()
                    .flat_map(|a| a.guard.iter().copied().chain(std::iter::once(a.body))),
            )
            .collect(),
        ast::ExprKind::Return(e) => e.iter().copied().collect(),
        ast::ExprKind::Block { tail, .. } => tail.iter().copied().collect(),
        _ => vec![],
    }
}
