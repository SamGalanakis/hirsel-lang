//! Causes of ownership judgments, read from the facts the flow pass already
//! holds: a loan's issue site names the temporary or binding that bounds a
//! value. Nothing here is computed for an accepted graph: diagnostics record a
//! pending explanation, and only a graph that reported one re-runs its transfer
//! in trace mode to recover the sites the lattice does not keep (moves and
//! field writes).
use super::*;
use crate::explain::{
    Cause, Explanation, Fact, Qualifier, Repair, Requirement, Sites, locality_repairs, qualifier,
    root_key, spell,
};

/// Sites logged by the trace pass of a failed graph.
#[derive(Default)]
pub(super) struct Trace {
    /// (binding, expression, node) for every move.
    moves: Vec<(usize, ExprId, usize)>,
    /// (binding, operation site, roots of the written value) for every write.
    writes: Vec<(usize, Span, Vec<usize>)>,
}
/// An explanation whose remaining sites need the trace.
pub(super) struct Pending {
    pub explanation: Explanation,
    /// The earliest issue site of a loan on a scope the value cannot outlive.
    pub temporary: Option<Site>,
    /// The moved binding and the node that read it.
    pub moved: Option<(usize, usize)>,
    /// A local binding holding content installed from `root`: (binding, root).
    pub installed: Option<(usize, usize)>,
}
impl From<Explanation> for Pending {
    fn from(explanation: Explanation) -> Self {
        Self {
            explanation,
            temporary: None,
            moved: None,
            installed: None,
        }
    }
}
/// The provided side of a locality failure.
pub(super) struct Provenance {
    pub facts: Vec<Fact<Cause>>,
    pub root: Option<(u8, usize)>,
    pub temporary: Option<Site>,
    pub installed: Option<(usize, usize)>,
    /// The binding at the root, when it is one.
    pub binding: Option<usize>,
}

impl Checker<'_> {
    pub(super) fn span_of(&self, expression: ExprId) -> Span {
        self.tree.expressions[expression.0 as usize].origin.span
    }
    pub(super) fn spelled(&self, expression: ExprId) -> Option<String> {
        spell(self.tree, self.symbols, expression)
    }
    pub(super) fn sites(&self) -> Sites<'_> {
        Sites {
            tree: self.tree,
            symbols: self.symbols,
        }
    }
    /// A binding's name and the site that declares it: a parameter's type,
    /// or the pattern that binds it.
    pub(super) fn binding_site(&self, index: usize) -> (String, Span) {
        let binding = &self.body.bindings[index];
        if let NameSite::Item(item) = binding.site {
            if let Some(item) = self
                .tree
                .items
                .get(item.0 as usize)
                .filter(|_| self.finalizer_self(index))
            {
                let name = match &item.kind {
                    hirsel_syntax::ItemKind::Type {
                        drop: Some(drop), ..
                    } => self.sites().name(drop.binding),
                    _ => self.sites().name(item.name),
                };
                return (name, item.name_span);
            }
        }
        self.sites().binding(binding)
    }
    /// Why the named storage is not global (or not owned): its passing mode,
    /// its enclosing frame, or this body's scope.
    fn binding_cause(&self, index: usize) -> (Cause, String, Span) {
        let binding = &self.body.bindings[index];
        let (name, span) = self.binding_site(index);
        let cause = match binding.binding {
            _ if binding.owner != self.graph.owner && self.graph.scoped => Cause::Capture,
            ResolvedName::Parameter(parameter) if binding.owner == self.graph.owner => {
                match self.parameter_passing(&binding.owner, parameter) {
                    PassingMode::Borrow => Cause::BorrowedParameter,
                    PassingMode::Mut => Cause::MutParameter,
                    PassingMode::Take => Cause::TakenParameter,
                }
            }
            _ if binding.owner != self.graph.owner => Cause::Capture,
            _ => Cause::Binding,
        };
        (cause, name, span)
    }
    /// The provided side of a value that is not global, from the value back
    /// to its root cause. The walk is bounded and picks the first root in
    /// a fixed order (borrowed, then sources and loans by binding index).
    pub(super) fn provenance(&self, state: &State, value: &Value) -> Provenance {
        let mut provenance = Provenance {
            facts: vec![],
            root: None,
            temporary: None,
            installed: None,
            binding: None,
        };
        let site = value.site.map(|e| self.span_of(e));
        if value.environment_local() {
            if let Some(site) = site {
                provenance
                    .facts
                    .push(Fact::new(Cause::Environment, site, &[]));
            }
            provenance.root = Some((1, value.site.map_or(0, |e| e.0 as usize)));
            return provenance;
        }
        if let Some(e) = value.site.filter(|e| {
            matches!(
                self.tree.expressions[e.0 as usize].kind,
                ExprKind::Call { .. }
            ) && self
                .source_rows
                .call(*e)
                .is_some_and(|i| !matches!(self.body.calls[i].target, CallTarget::Constructor(_)))
        }) {
            if let ExprKind::Call { callee, .. } = self.tree.expressions[e.0 as usize].kind {
                if let Some(callee) = self.spelled(callee) {
                    let local = qualifier(Qualifier::In(&[".."]));
                    if !value.roots().is_empty() {
                        provenance.facts.push(Fact::new(
                            Cause::LocalResult,
                            self.span_of(e),
                            &[&callee, &local],
                        ));
                    }
                }
            }
        }
        let mut current = value.clone();
        let mut seen = BTreeSet::new();
        for _ in 0..6 {
            let Some(index) = current.roots().into_iter().find(|r| seen.insert(*r)) else {
                break;
            };
            let (cause, name, span) = self.binding_cause(index);
            let slot = &state.slots[index].value;
            let deeper = cause == Cause::Binding
                && !self.arenas.global(&slot.modes.relation)
                && slot.roots().iter().any(|r| !seen.contains(r));
            if deeper {
                provenance
                    .facts
                    .push(Fact::new(Cause::Holds, span, &[&name]));
                let next = slot
                    .roots()
                    .into_iter()
                    .find(|r| !seen.contains(r))
                    .unwrap();
                if slot.content.parts.is_some() && provenance.installed.is_none() {
                    provenance.installed = Some((index, next));
                }
                current = slot.clone();
                continue;
            }
            provenance.facts.push(Fact::new(cause, span, &[&name]));
            provenance.root = Some((0, index));
            provenance.binding = Some(index);
            return provenance;
        }
        let content = value.content.summary();
        if let Some((_, scope, _)) = content.scopes().next() {
            provenance.temporary = content.scopes().map(|(_, _, site)| site).min();
            provenance.root = Some((2, scope));
            return provenance;
        }
        if value.unplaced() {
            if let Some(site) = site {
                provenance.facts.push(Fact::new(Cause::Unplaced, site, &[]));
            }
            return provenance;
        }
        if let Some(site) = site {
            if provenance.facts.is_empty() {
                provenance
                    .facts
                    .push(Fact::new(Cause::LocalValue, site, &[]));
            }
        }
        provenance
    }
    /// The function result's requirement, at its written type or the function.
    pub(super) fn result_requirement(&self, result: &Modes) -> Fact<Requirement> {
        let sites = self.sites();
        let syntax = sites.signature(self.graph.owner);
        let span = sites.result(self.graph.owner);
        match result.relation.named_parameters() {
            Some(sources) => {
                let names: Vec<String> = sources
                    .iter()
                    .filter_map(|source| {
                        let parameter = match source {
                            ParameterRef::Bound {
                                depth: 0,
                                parameter,
                            } => *parameter,
                            ParameterRef::Body { parameter, .. } => *parameter,
                            _ => return None,
                        };
                        syntax
                            .and_then(|s| s.parameters.get(parameter.0 as usize))
                            .and_then(|p| p.name)
                            .map(|n| sites.name(n))
                    })
                    .collect();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                Fact::new(
                    Requirement::ResultSources,
                    span,
                    &["", &qualifier(Qualifier::In(&names))],
                )
            }
            _ => Fact::new(
                Requirement::ResultGlobal,
                span,
                &["", &qualifier(Qualifier::In(&[".."]))],
            ),
        }
    }
    /// The receiving contract of storage that only takes global values.
    pub(super) fn receiver_requirement(&self, index: usize) -> Fact<Requirement> {
        let (name, span) = self.binding_site(index);
        let binding = &self.body.bindings[index];
        if matches!(binding.binding, ResolvedName::Parameter(_)) && binding.usage == UseKind::Mut {
            return Fact::new(Requirement::MutStorage, span, &[&name]);
        }
        let annotated = match binding.site {
            NameSite::Pattern(pattern) => self
                .pattern_indices
                .get(&pattern)
                .is_some_and(|&row| self.body.patterns[row].annotation.is_some()),
            _ => false,
        };
        if annotated {
            Fact::new(Requirement::AnnotatedBinding, span, &[&name])
        } else {
            Fact::new(
                Requirement::GlobalBinding,
                span,
                &[&name, &qualifier(Qualifier::In(&[&name]))],
            )
        }
    }
    /// A two-sided locality explanation with the verified repairs for its
    /// cause and requirement (moonshot-diagnostics, verify-moon2).
    pub(super) fn locality(
        &self,
        state: &State,
        value: &Value,
        required: Fact<Requirement>,
    ) -> Pending {
        let provenance = self.provenance(state, value);
        let mut explanation = Explanation {
            provided: provenance.facts,
            root: provenance
                .root
                .map(|root| root_key(&self.own.definition, ("locality", root))),
            ..Explanation::default()
        };
        if let (Some(cause), Some(index)) = (explanation.cause().cloned(), provenance.binding) {
            let shared = self.properties[self.body.bindings[index].ty.index()].shared;
            let x = cause.names.first().cloned().unwrap_or_default();
            let (repairs, cautions) = locality_repairs(cause.kind, required.kind, &x, shared);
            explanation.repairs = repairs;
            explanation.cautions = cautions;
        }
        explanation.required = Some(required);
        Pending {
            explanation,
            temporary: provenance.temporary,
            moved: None,
            installed: provenance.installed,
        }
    }
    /// A temporary result of a closure literal call, read without the closure
    /// or deep copied (probes 8a, 8g).
    pub(super) fn environment_repairs(&self, value: &Value, pending: &mut Pending) {
        let Some(call) = value.site else {
            return;
        };
        let ExprKind::Call { callee, .. } = self.tree.expressions[call.0 as usize].kind else {
            return;
        };
        let ExprKind::Closure { body, .. } = self.tree.expressions[callee.0 as usize].kind else {
            return;
        };
        let tail = match &self.tree.expressions[body.0 as usize].kind {
            ExprKind::Block {
                statements,
                tail: Some(tail),
            } if statements.is_empty() => *tail,
            _ => body,
        };
        if let Some(read) = self.spelled(tail) {
            pending
                .explanation
                .repairs
                .push(Repair::ReadDirectly(format!("`{read}`")));
        }
        pending
            .explanation
            .repairs
            .push(Repair::DeepCopy("(fn() { .. })()".into()));
    }
    /// A result that depends on a temporary of this body (MODE-1). A call of
    /// a closure literal is local to that closure's environment.
    pub(super) fn temporary_result(&self, value: &Value, result: Span) -> Pending {
        let content = value.content.summary();
        let mut pending: Pending = Explanation {
            required: Some(Fact::new(Requirement::ResultNoTemporary, result, &[])),
            root: content
                .scopes()
                .next()
                .map(|(_, scope, _)| root_key(&self.own.definition, ("locality", (2u8, scope)))),
            ..Explanation::default()
        }
        .into();
        pending.temporary = content.scopes().map(|(_, _, site)| site).min();
        let callee =
            value
                .site
                .and_then(|call| match self.tree.expressions[call.0 as usize].kind {
                    ExprKind::Call { callee, .. } => Some(callee),
                    _ => None,
                });
        if let Some(callee) = callee.filter(|callee| {
            matches!(
                self.tree.expressions[callee.0 as usize].kind,
                ExprKind::Closure { .. }
            )
        }) {
            pending.explanation.provided.push(Fact::new(
                Cause::Environment,
                self.span_of(callee),
                &[],
            ));
            self.environment_repairs(value, &mut pending);
        }
        pending
    }
    /// A plain two-sided explanation over a binding's cause.
    pub(super) fn binding_explanation(
        &self,
        index: usize,
        cause: Option<Cause>,
        required: Fact<Requirement>,
    ) -> Pending {
        let (default, name, span) = self.binding_cause(index);
        Explanation::default()
            .provided(Fact::new(cause.unwrap_or(default), span, &[&name]))
            .required(required)
            .into()
    }
    /// A use of a moved binding: the move site comes from the trace.
    pub(super) fn moved(&self, index: usize, read: ExprId, required: Fact<Requirement>) -> Pending {
        let (name, _) = self.binding_site(index);
        let mut pending: Pending = Explanation {
            required: Some(required),
            root: Some(root_key(&self.own.definition, ("moved", index))),
            provided: vec![Fact::new(
                Cause::Uninitialised,
                self.span_of(read),
                &[&name],
            )],
            ..Explanation::default()
        }
        .into();
        pending.moved = Some((index, self.node));
        pending
    }
    /// The live holder of a loan on `place`, if one is in a binding: the
    /// value the loan set says still uses it (LANG-3).
    pub(super) fn lent(
        &self,
        state: &State,
        place: &Place,
        required: Fact<Requirement>,
    ) -> Pending {
        let (name, span) = self.binding_site(place.binding);
        if let Some(source) = self.iteration(state, place) {
            let site = self.tree.expressions[source.0 as usize].origin.span;
            return Explanation::default()
                .provided(Fact::new(Cause::Iterated, site, &[&name]))
                .required(required)
                .into();
        }
        let holder = self
            .lender(state, place)
            .or_else(|| self.capturer(state, place.binding))
            .flatten();
        let provided = match holder {
            Some(index) => {
                let (holder, at) = self.binding_site(index);
                Fact::new(Cause::Lent, at, &[&name, &holder])
            }
            None => Fact::new(Cause::Lent, span, &[&name, "a live result"]),
        };
        Explanation::default()
            .provided(provided)
            .required(required)
            .into()
    }
    /// Record a diagnostic and its explanation, rendered after the trace.
    pub(super) fn report(
        &mut self,
        rule: &str,
        site: Span,
        message: &str,
        explain: impl FnOnce(&Self) -> Pending,
    ) -> Option<&mut Diagnostic> {
        if !self.emit {
            return None;
        }
        let pending = explain(self);
        self.diagnostics
            .push(Diagnostic::error(RuleId::new(rule).unwrap(), site, message));
        self.explanations
            .push((self.diagnostics.len() - 1, pending));
        self.diagnostics.last_mut()
    }
    /// Trace-pass logging; free when no graph has failed.
    pub(super) fn log_move(&mut self, binding: usize, expression: ExprId) {
        let node = self.node;
        if let Some(trace) = &mut self.trace {
            trace.moves.push((binding, expression, node));
        }
    }
    pub(super) fn log_write(&mut self, binding: usize, site: Span, value: &Value) {
        if self.trace.is_some() {
            let roots = value.roots();
            self.trace
                .as_mut()
                .unwrap()
                .writes
                .push((binding, site, roots));
        }
    }
    fn needs_trace(&self) -> bool {
        self.explanations
            .iter()
            .any(|(_, p)| p.moved.is_some() || p.installed.is_some())
    }
    /// Recover trace sites for this graph's explanations, then render them.
    pub(super) fn explain(&mut self, initial: State, live: &[BTreeSet<Holder>]) {
        if self.explanations.is_empty() {
            return;
        }
        if self.needs_trace() {
            let explanations = std::mem::take(&mut self.explanations);
            self.emit = false;
            let inputs = solve(self, initial, live);
            self.trace = Some(Trace::default());
            for (id, input) in inputs.into_iter().enumerate() {
                if let Some(mut input) = input {
                    self.node = id;
                    let graph = self.graph;
                    self.transfer(&graph.nodes[id].op, &mut input);
                }
            }
            self.explanations = explanations;
        }
        let trace = self.trace.take().unwrap_or_default();
        // One failed obligation reached through several parts is reported once,
        // as it was before explanations told the reports apart.
        let duplicates: BTreeSet<usize> = self
            .explanations
            .iter()
            .map(|(index, _)| *index)
            .filter(|&index| {
                self.diagnostics[..index]
                    .iter()
                    .any(|earlier| *earlier == self.diagnostics[index])
            })
            .collect();
        for (index, mut pending) in std::mem::take(&mut self.explanations) {
            if let Some(site) = pending.temporary {
                let fact = match site {
                    Site::Expression(e) => Fact::new(Cause::Temporary, self.span_of(e), &[]),
                    Site::Binding(binding) => {
                        let (name, span) = self.binding_site(binding);
                        Fact::new(Cause::Binding, span, &[&name])
                    }
                    Site::Frame => {
                        Fact::new(Cause::FrameStorage, self.diagnostics[index].primary, &[])
                    }
                };
                pending.explanation.provided.push(fact);
            }
            if let Some((holder, root)) = pending.installed {
                if let Some((_, site, _)) = trace
                    .writes
                    .iter()
                    .filter(|(binding, _, roots)| *binding == holder && roots.contains(&root))
                    .min_by_key(|(_, site, _)| site.start)
                {
                    let (name, _) = self.binding_site(holder);
                    if let Some(fact) = pending
                        .explanation
                        .provided
                        .iter_mut()
                        .find(|f| f.kind == Cause::Holds && f.names[0] == name)
                    {
                        fact.span = *site;
                    }
                }
            }
            if let Some((binding, node)) = pending.moved {
                if let Some(at) = self.move_site(&trace, binding, node) {
                    let (name, _) = self.binding_site(binding);
                    pending.explanation.provided = vec![Fact::new(Cause::Moved, at, &[&name])];
                }
            }
            if pending.explanation.cause().is_none() {
                pending.explanation.provided.push(Fact::new(
                    Cause::LocalValue,
                    self.diagnostics[index].primary,
                    &[],
                ));
            }
            pending.explanation.render(&mut self.diagnostics[index]);
        }
        let mut index = 0;
        self.diagnostics.retain(|_| {
            index += 1;
            !duplicates.contains(&(index - 1))
        });
    }
    /// The first move of `binding` from which the reading node is reachable.
    fn move_site(&self, trace: &Trace, binding: usize, target: usize) -> Option<Span> {
        trace
            .moves
            .iter()
            .filter(|(moved, _, _)| *moved == binding)
            .filter(|(_, _, node)| {
                let mut seen = BTreeSet::new();
                let mut pending: Vec<usize> = self.graph.nodes[*node].next.clone();
                while let Some(id) = pending.pop() {
                    if id == target {
                        return true;
                    }
                    if seen.insert(id) {
                        pending.extend(&self.graph.nodes[id].next);
                    }
                }
                false
            })
            .map(|(_, expression, _)| self.span_of(*expression))
            .min_by_key(|span| span.start)
    }
}

impl Checker<'_> {
    /// R6: a `copy` passed to a borrowed parameter whose result keeps nothing
    /// is redundant when its binding is never used again: the borrow reads the
    /// same value, and nothing later observes the copy's sharing. Reported only
    /// where that is certain from syntax and the graph, as a warning (OWN-4).
    pub(super) fn redundant_copy(&mut self, callee: ExprId, argument: ExprId, others: &[ExprId]) {
        let ExprKind::Unary {
            op: hirsel_syntax::UnaryOp::Copy,
            value: child,
        } = self.tree.expressions[argument.0 as usize].kind
        else {
            return;
        };
        let Some(copied) = place(self.body, child, self.source_rows) else {
            return;
        };
        let binding = copied.binding;
        if !copied.projections.is_empty() || self.body.bindings[binding].owner != self.graph.owner {
            return;
        }
        let Some(names) = self.unused_after(binding, argument) else {
            return;
        };
        // Another argument naming it could conflict with a borrow (OWN-3).
        if others
            .iter()
            .any(|other| *other != argument && self.names_within(*other, &names))
        {
            return;
        }
        let (name, _) = self.binding_site(binding);
        let callee_name = self
            .spelled(callee)
            .unwrap_or_else(|| crate::explain::prose("the callee"));
        let span = self.span_of(argument);
        let mut diagnostic = Diagnostic::error(
            RuleId::new("OWN-4").unwrap(),
            span,
            "this `copy` is redundant",
        );
        diagnostic.severity = hirsel_diag::Severity::Warning;
        Explanation {
            repairs: vec![Repair::PassBorrowed(name.clone())],
            ..Explanation::default()
        }
        .provided(Fact::new(Cause::LastUse, self.span_of(child), &[&name]))
        .required(Fact::new(
            Requirement::Borrows,
            self.span_of(callee),
            &[&callee_name],
        ))
        .render(&mut diagnostic);
        self.diagnostics.push(diagnostic);
    }
}
impl Checker<'_> {
    /// The expressions naming `binding`, when no node reachable from the
    /// current one names it and no closure captures it: `use` is its last use.
    pub(super) fn unused_after(&self, binding: usize, use_: ExprId) -> Option<BTreeSet<ExprId>> {
        let names: BTreeSet<ExprId> = self
            .body
            .expressions
            .iter()
            .map(|row| row.expression)
            .filter(|e| {
                matches!(self.tree.expressions[e.0 as usize].kind, ExprKind::Path(_))
                    && place(self.body, *e, self.source_rows).is_some_and(|p| p.binding == binding)
            })
            .collect();
        let owner = self.body.bindings[binding].owner;
        let captured = self.body.closures.iter().any(|closure| {
            closure.captures.iter().any(|capture| {
                self.source_rows
                    .binding(self.body, Some(owner), &capture.binding)
                    == Some(binding)
                    || self.source_rows.binding(self.body, None, &capture.binding) == Some(binding)
            })
        });
        if captured {
            return None;
        }
        let graph = self.graph;
        let mentions = |id: usize| {
            let (reads, writes) = reads_writes(&graph.nodes[id].op);
            let place_write = match &graph.nodes[id].op {
                Op::Write { place: p, .. } => Some(*p),
                _ => None,
            };
            reads
                .iter()
                .chain(&writes)
                .chain(&place_write)
                .any(|e| names.contains(e) || (*e != use_ && within_shallow(self, *e, &names)))
        };
        let mut seen = BTreeSet::new();
        let mut pending: Vec<usize> = graph.nodes[self.node].next.clone();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            if mentions(id) {
                return None;
            }
            pending.extend(&graph.nodes[id].next);
        }
        Some(names)
    }
    fn names_within(&self, root: ExprId, names: &BTreeSet<ExprId>) -> bool {
        let mut pending = vec![root];
        while let Some(e) = pending.pop() {
            if names.contains(&e) {
                return true;
            }
            pending.extend(crate::expression_children(self.tree, e));
        }
        false
    }
}
/// Whether a node's expression is a wrapper (a borrow, a field read) of a name.
/// The temporaries an operation reads and those it writes.
fn reads_writes(op: &Op) -> (Vec<ExprId>, Vec<ExprId>) {
    match op {
        Op::Use { expression, .. } | Op::Capture { expression, .. } => (vec![], vec![*expression]),
        Op::Finish {
            expression,
            children,
            ..
        } => (children.clone(), vec![*expression]),
        Op::Forward { expression, child } => (vec![*child], vec![*expression]),
        Op::IterationLoan { source, .. } => (vec![*source], vec![]),
        Op::ArgumentLoan { argument, .. } => (vec![*argument], vec![]),
        Op::Bind { value, .. } | Op::Return { value, .. } => (vec![*value], vec![]),
        Op::Write { value, .. } => (vec![*value], vec![]),
        Op::Call {
            expression,
            callee,
            arguments,
            ..
        } => (
            std::iter::once(*callee)
                .chain(arguments.iter().map(|a| a.value))
                .collect(),
            vec![*expression],
        ),
        _ => (vec![], vec![]),
    }
}
fn within_shallow(checker: &Checker<'_>, e: ExprId, names: &BTreeSet<ExprId>) -> bool {
    let mut current = e;
    for _ in 0..8 {
        match checker.tree.expressions[current.0 as usize].kind {
            ExprKind::Unary { value, .. } => current = value,
            ExprKind::Field { base, .. } => current = base,
            ExprKind::Index { base, .. } => current = base,
            _ => break,
        }
        if names.contains(&current) {
            return true;
        }
    }
    false
}
