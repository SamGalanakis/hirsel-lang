//! Causes of ownership judgments, read from the facts the flow pass already
//! holds: a loan's issue site names the temporary or binding that bounds a
//! value. Nothing here is computed for an accepted graph: diagnostics record a
//! pending explanation, and only a graph that reported one re-runs its transfer
//! in trace mode to recover the sites the lattice does not keep (moves and
//! field writes).
use super::*;
use hirsel_modes::explain::{
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
    uniqueness: BTreeMap<(usize, usize), UniqueStep>,
}
#[derive(Default)]
struct UniqueStep {
    /// A restored/replaced/dead value cuts the backwards walk.
    cut: bool,
    /// A later possible share must not hide an earlier loss when the input
    /// was already aliased (including at a loop's fixed point).
    already_aliased: bool,
    losses: Vec<(Span, String)>,
    /// Keep related-call release guidance separate from ordinary copies.
    related_result: bool,
    repairs: Vec<Repair>,
    items: Vec<Fact<Cause>>,
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
    /// The storage and failed node, for diagnostic-only uniqueness tracing.
    pub uniqueness: Option<(usize, usize)>,
}
impl From<Explanation> for Pending {
    fn from(explanation: Explanation) -> Self {
        Self {
            explanation,
            temporary: None,
            moved: None,
            installed: None,
            uniqueness: None,
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

impl Checker<'_, '_> {
    /// Inspect a transfer only in a failed graph's replay. These are candidate
    /// losses, not new flow facts: loop inputs may already be aliased.
    fn uniqueness_step(
        &self,
        binding: usize,
        op: &Op,
        before: &State,
        after: &State,
    ) -> UniqueStep {
        let (name, _) = self.binding_site(binding);
        let old = &before.slots[binding].value;
        let new = &after.slots[binding].value;
        let mut step = UniqueStep {
            cut: before.slots[binding].available != LIVE
                || after.slots[binding].available != LIVE
                || new.modes.uniqueness == Uniqueness::Unique,
            already_aliased: old.modes.uniqueness != Uniqueness::Unique,
            ..UniqueStep::default()
        };
        if step.cut {
            return step;
        }
        match op {
            Op::Use { expression, .. } => {
                if let Some(source) = place(self.body, *expression, self.source_rows) {
                    let mut owners = BTreeSet::from([source.binding]);
                    let mut next = before.slots[source.binding].value.element_owner();
                    while let Some(owner) = next.filter(|owner| owners.insert(*owner)) {
                        next = before.slots[owner].value.element_owner();
                    }
                    if owners.contains(&binding)
                        && self.value(after, *expression).usage == UseKind::Copy
                        && self
                            .property(*expression)
                            .is_some_and(|p| !p.plain && !p.flat && !p.shared)
                    {
                        let span = self
                            .tree
                            .expressions
                            .iter()
                            .find_map(|e| match e.kind {
                                ExprKind::Unary {
                                    op: UnaryOp::Copy,
                                    value,
                                } if value == *expression => Some(e.origin.span),
                                _ => None,
                            })
                            .unwrap_or_else(|| self.span_of(*expression));
                        let part = self.spelled(*expression).unwrap_or_else(|| name.clone());
                        step.losses.push((
                            span,
                            format!(
                                "copying `{part}` shares counted storage reachable from `{name}`"
                            ),
                        ));
                    }
                }
            }
            Op::Call {
                expression,
                callee,
                arguments,
                requirements,
                ..
            } => {
                let result = self.value(after, *expression);
                if self
                    .property(*expression)
                    .is_some_and(|p| !p.plain && !p.flat && !p.shared)
                    && result
                        .content
                        .summary()
                        .bindings(Kind::Read)
                        .any(|root| root == binding)
                {
                    step.related_result = true;
                    let description = if self.callee_definition(*callee).is_none() {
                        format!(
                            "the function-value call has an escaped origin; its counted result may share `{name}` permanently"
                        )
                    } else {
                        format!(
                            "the counted result of this call may share `{name}`; release of its confined holder and every derived holder remains unproved"
                        )
                    };
                    step.losses.push((self.span_of(*expression), description));
                }
                for (argument, requirement) in arguments.iter().zip(requirements) {
                    let Some(requirement) = requirement else {
                        continue;
                    };
                    if requirement.passing == PassingMode::Take
                        && self
                            .reached_bindings(before, &self.value(before, argument.value))
                            .contains(&binding)
                    {
                        step.losses.push((self.span_of(argument.value), format!("this taken result escapes the confined borrow of `{name}`; an external counted holder remains unproved")));
                    }
                    if requirement.passing != PassingMode::Mut
                        || requirement.modes.uniqueness == Uniqueness::Unique
                        || self.callee_preserves_state(*callee, requirement.parameter, false)
                        || !place(self.body, argument.value, self.source_rows).is_some_and(|p| {
                            p.binding == binding
                                && (p.projections.is_empty()
                                    || !self
                                        .property(argument.value)
                                        .is_some_and(|p| p.plain || p.shared))
                        })
                    {
                        continue;
                    }
                    if !self.callee_preserves_unique(*callee, requirement.parameter) {
                        let part = self.spelled(argument.value).unwrap_or_else(|| name.clone());
                        step.losses.push((self.span_of(*expression), format!("this plain `mut` handback through `{part}` may install counted shares in `{name}`")));
                        if let Some(path) = self.callee_definition(*callee)
                            && hirsel_types::unique_summary::same_module(
                                path,
                                self.own.definition.path(),
                            )
                            && let Some(failure) = self.signatures.unique_failure(
                                path,
                                UniqueClaim::Parameter(ParamId(requirement.parameter as u32)),
                            )
                        {
                            step.items.push(Fact::new(
                                Cause::UniquenessLoss,
                                failure.span,
                                &[
                                    &name,
                                    &format!(
                                        "`{}` no longer hands this parameter back unique: {}",
                                        path.0.name, failure.message
                                    ),
                                ],
                            ));
                        }
                        // Consult only the written direct signature. A body in
                        // another module or a foreign/bodyless declaration is
                        // not a local declaration edit we can recommend.
                        if !step.already_aliased
                            && step.losses.len() == 1
                            && let Some(repair) =
                                self.helper_unique_repair(*callee, requirement.parameter)
                        {
                            step.repairs.push(repair);
                        }
                    } else {
                        for (input, row) in arguments.iter().zip(requirements) {
                            let Some(row) = row else { continue };
                            let ledger = self
                                .callee_definition(*callee)
                                .is_some_and(|path| path.package_is("core", "1"));
                            let supplier = match row.passing {
                                PassingMode::Take => true,
                                PassingMode::Mut => ledger,
                                PassingMode::Borrow => {
                                    ledger && !self.callee_unretained(*callee, row.parameter)
                                }
                            };
                            if !supplier
                                || input.value == argument.value
                                || self
                                    .property(input.value)
                                    .is_some_and(|p| p.plain || p.handle_root)
                                || self.entry_value(before, input.value).modes.uniqueness
                                    == Uniqueness::Unique
                            {
                                continue;
                            }
                            // Name the written parameter rather than guessing a
                            // Map key/value position from a function's spelling.
                            let parameter = self
                                .ty(*callee)
                                .and_then(|t| self.t.get(t))
                                .and_then(|t| match &t.kind {
                                    TypeKind::Function(f) => {
                                        f.parameters.get(row.parameter).and_then(|p| p.name.clone())
                                    }
                                    _ => None,
                                })
                                .unwrap_or_else(|| format!("argument {}", row.parameter + 1));
                            let source = self.aliased_source(before, input.value);
                            let site = if matches!(
                                self.tree.expressions[input.value.0 as usize].kind,
                                ExprKind::Unary {
                                    op: UnaryOp::Copy | UnaryOp::Take,
                                    ..
                                }
                            ) {
                                input.value
                            } else {
                                source
                            };
                            step.losses.push((self.span_of(site), format!("the `{parameter}` input may share counted storage; this call therefore does not hand `{name}` back deeply unique")));
                            self.iteration_item(before, source, &mut step);
                        }
                    }
                }
            }
            Op::Write {
                place: target,
                value,
                operation,
                ..
            } => {
                if let Some(target) =
                    place(self.body, *target, self.source_rows).filter(|p| p.binding == binding)
                {
                    if !target.projections.is_empty() && old.modes.uniqueness == Uniqueness::Unique
                    {
                        step.losses.push((
                            self.span_of(*value),
                            format!(
                                "this stored part may share counted storage reachable from `{name}`"
                            ),
                        ));
                    } else if target.projections.is_empty() {
                        // A replacement starts a new value's history. An
                        // unproved RHS is not evidence against its initializer.
                        step.cut = true;
                        step.losses.push((
                            *operation,
                            format!("this replacement does not prove `{name}` deeply unique"),
                        ));
                    }
                }
            }
            _ => {}
        }
        step
    }

    /// Add context to an existing loss, never infer an element promise from
    /// the collection's root or its source marker. Only indexed sequences have
    /// this contract; associated cursors supply their own checked item rows.
    fn iteration_item(&self, state: &State, mut source: ExprId, step: &mut UniqueStep) {
        while let ExprKind::Unary {
            op: UnaryOp::Copy | UnaryOp::Take,
            value,
        } = self.tree.expressions[source.0 as usize].kind
        {
            source = value;
        }
        let Some(place) = place(self.body, source, self.source_rows) else {
            return;
        };
        if !place.projections.is_empty() {
            return;
        }
        let binding = &self.body.bindings[place.binding];
        let NameSite::Pattern(pattern) = binding.site else {
            return;
        };
        let item_span = self.tree.patterns[pattern.0 as usize].origin.span;
        let Some(loop_source) = self.tree.statements.iter().find_map(|s| {
            let hirsel_syntax::StmtKind::For {
                pattern, source, ..
            } = s.kind
            else {
                return None;
            };
            let span = self.tree.patterns[pattern.0 as usize].origin.span;
            (span.start <= item_span.start && span.end >= item_span.end).then_some(source)
        }) else {
            return;
        };
        if !self
            .ty(loop_source)
            .and_then(|t| self.t.get(t))
            .is_some_and(|t| {
                matches!(
                    t.kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::List | BuiltinType::Array,
                        ..
                    }
                )
            })
        {
            return;
        }
        let (item, _) = self.binding_site(place.binding);
        let declared = self.t.get(binding.ty).unwrap();
        let current = &state.slots[place.binding].value;
        let mode = |u| {
            if u == Uniqueness::Unique {
                "unique"
            } else {
                "not proved unique"
            }
        };
        let description = format!(
            "loop item `{item}` has declared item uniqueness: {}; current item uniqueness at this input: {}. The item contract is separate from the sequence's root uniqueness",
            mode(declared.modes.uniqueness),
            mode(current.modes.uniqueness)
        );
        step.items.push(Fact::new(
            Cause::IterationItem,
            item_span,
            &[&item, &description],
        ));
        let access = match self.tree.expressions[loop_source.0 as usize].kind {
            ExprKind::Unary {
                op: UnaryOp::Take, ..
            } => "`take` consumes the sequence; it does not prove arbitrary nested elements unique",
            ExprKind::Unary {
                op: UnaryOp::Mut, ..
            } => {
                "`mut` iterates element places with checked mutable access; it does not strengthen their declared uniqueness"
            }
            _ => "this source is borrowed; its items use the checked borrowed element contract",
        };
        step.items.push(Fact::new(
            Cause::IterationItem,
            self.span_of(loop_source),
            &[&item, access],
        ));
        // Deliberately narrow: a closed String/Bytes/scalar/List tree has no
        // hidden resource, generic contract, callable capture or placement.
        let noalloc = self
            .graph_callable()
            .is_some_and(|(f, _)| f.allocation == hirsel_types::Allocation::NoAlloc);
        let independent_copy = !noalloc && self.independent_capture_copy(current, binding.ty);
        step.repairs.push(Repair::IterationItem {
            item,
            independent_copy,
        });
    }

    fn helper_unique_repair(&self, callee: ExprId, ordinal: usize) -> Option<Repair> {
        let path = self.callee_definition(callee)?;
        let owner = &self.syntax.item.definition;
        if path.0.package != owner.0.package
            || path.0.modules != owner.0.modules
            || path.package_is("core", "1")
        {
            return None;
        }
        let TypeKind::Function(function) = &self.t.get(self.ty(callee)?)?.kind else {
            return None;
        };
        if function.foreign.is_some() {
            return None;
        }
        if self
            .signatures
            .cached(DefId::intern(path))?
            .metadata
            .visibility
            == hirsel_syntax::Visibility::Private
        {
            return None;
        }
        let parameter = function.parameters.get(ordinal)?;
        Some(Repair::PreserveUnique {
            helper: path.0.name.clone(),
            parameter: parameter.name.clone()?,
        })
    }

    /// A consumed argument is checked at its call after its storage was moved.
    /// Start its diagnostic trace at the read, before the move cuts history.
    pub(super) fn uniqueness_origin(&self, mut expression: ExprId) -> Option<(usize, usize)> {
        let mut consumed = false;
        while let ExprKind::Unary {
            op: UnaryOp::Take,
            value,
        } = self.tree.expressions[expression.0 as usize].kind
        {
            expression = value;
            consumed = true;
        }
        let place = place(self.body, expression, self.source_rows)?;
        let node = if consumed {
            self.graph.nodes.iter().position(|node| {
                matches!(node.op,
                Op::Use { expression: e, .. } if e == expression)
            })?
        } else {
            self.node
        };
        Some((place.binding, node))
    }

    fn reachable(&self, from: usize, target: usize) -> bool {
        let mut seen = BTreeSet::new();
        let mut pending = self.graph.nodes[from].next.clone();
        while let Some(node) = pending.pop() {
            if node == target {
                return true;
            }
            if seen.insert(node) {
                pending.extend(&self.graph.nodes[node].next);
            }
        }
        false
    }

    /// Walk only aliased predecessors of the failed input, stopping at a
    /// restoration or replacement. Multiple paths remain explicitly possible.
    fn uniqueness_causes(
        &self,
        trace: &Trace,
        binding: usize,
        target: usize,
    ) -> (Vec<Fact<Cause>>, Vec<Repair>, bool) {
        let (name, span) = self.binding_site(binding);
        let mut predecessors = vec![vec![]; self.graph.nodes.len()];
        for (id, node) in self.graph.nodes.iter().enumerate() {
            for &next in &node.next {
                predecessors[next].push(id);
            }
        }
        let mut pending = predecessors[target].clone();
        let mut seen = BTreeSet::new();
        let mut related_result = false;
        let mut losses = vec![];
        let mut repairs = vec![];
        let mut items = vec![];
        let mut looped = false;
        let mut uncertain = false;
        let mut joined = false;
        while let Some(node) = pending.pop() {
            if !seen.insert(node) {
                continue;
            }
            let Some(step) = trace.uniqueness.get(&(binding, node)) else {
                continue;
            };
            joined |= predecessors[node].len() > 1;
            if !step.losses.is_empty() {
                looped |= self.reachable(target, node);
                uncertain |= step.already_aliased;
                related_result |= step.related_result;
                losses.extend(step.losses.iter().cloned());
                repairs.extend(step.repairs.iter().cloned());
                items.extend(step.items.iter().cloned());
                if !step.already_aliased {
                    continue;
                }
            }
            if !step.cut {
                pending.extend(&predecessors[node]);
            }
        }
        losses.sort_by_key(|(span, description)| (span.start, span.end, description.clone()));
        losses.dedup();
        let merged = looped || uncertain || losses.len() > 1 || losses.is_empty() && joined;
        // A declaration promise repairs one handback, not a surviving copy or
        // other share on the path. Keep every loss label, but do not present
        // this recipe as a repair for mixed or uncertain causes.
        if merged {
            repairs.retain(|r| matches!(r, Repair::IterationItem { .. }));
        }
        items.sort_by_key(|f| (f.span.start, f.span.end, f.names.clone()));
        items.dedup();
        let mut facts: Vec<_> = items
            .into_iter()
            .chain(losses.into_iter().map(|(span, description)| {
                Fact::new(Cause::UniquenessLoss, span, &[&name, &description])
            }))
            .collect();
        if merged {
            let description = if looped {
                format!(
                    "`{name}` reaches this merge with possible sharing from a previous iteration; the initializer alone does not establish uniqueness here"
                )
            } else if joined {
                format!(
                    "control-flow paths merge without one proved loss operation for `{name}`; its current value is not proved deeply unique"
                )
            } else {
                format!(
                    "the incoming value of `{name}` was already not proved unique; these later possible shares do not establish one original loss operation"
                )
            };
            facts.push(Fact::new(
                Cause::UniquenessMerge,
                self.graph.nodes[target].span,
                &[&name, &description],
            ));
        }
        if facts.is_empty() {
            facts.push(Fact::new(Cause::Aliased, span, &[&name]));
        }
        (facts, repairs, related_result)
    }

    pub(super) fn span_of(&self, expression: ExprId) -> Span {
        self.tree.expressions[expression.0 as usize].origin.span
    }
    pub(super) fn spelled(&self, expression: ExprId) -> Option<String> {
        spell(self.tree, expression)
    }
    pub(super) fn sites(&self) -> Sites<'_> {
        Sites {
            syntax: self.syntax,
        }
    }
    /// Recover an owned call behind immutable local bindings and pattern
    /// payloads. `site` on a read is the read itself; the slot retains the
    /// producing expression. This is diagnostic-only: never follow lifetime
    /// roots (a result related to an argument is still owned), copy boundaries,
    /// or mutable storage whose historical value may have been replaced.
    pub(super) fn owned_call_source(&self, state: &State, mut source: ExprId) -> Option<ExprId> {
        let mut seen = BTreeSet::new();
        while seen.insert(source) {
            match self.tree.expressions[source.0 as usize].kind {
                ExprKind::Call { .. } => {
                    return self.source_rows.call(source).and_then(|i| {
                        (!matches!(self.body.calls[i].target, CallTarget::Constructor(_)))
                            .then_some(source)
                    });
                }
                ExprKind::Unary {
                    op: UnaryOp::Take,
                    value,
                } => source = value,
                _ => {
                    let place = place(self.body, source, self.source_rows)?;
                    let binding = &self.body.bindings[place.binding];
                    if binding.mutable || binding.owner != self.graph.owner {
                        return None;
                    }
                    source = state.slots[place.binding].value.site?;
                }
            }
        }
        None
    }
    /// A binding's name and the site that declares it: a parameter's type,
    /// or the pattern that binds it.
    pub(super) fn binding_site(&self, index: usize) -> (String, Span) {
        let binding = &self.body.bindings[index];
        if let NameSite::Item = binding.site {
            if let Some(item) = Some(&self.syntax.item).filter(|_| self.finalizer_self(index)) {
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
            if cause == Cause::Capture && self.graph.scoped {
                if let NameSite::Expression(e) = self.graph.owner {
                    provenance.facts.push(Fact::new(
                        Cause::BorrowedCapture,
                        self.span_of(e),
                        &[&name],
                    ));
                }
                // Keep the source owner's declaration distinct from the
                // closure that lends it. Both are existing checked sites.
                let owner = match binding_mode(self, index) {
                    Some(PassingMode::Borrow) => Cause::BorrowedParameter,
                    Some(PassingMode::Mut) => Cause::MutParameter,
                    Some(PassingMode::Take) => Cause::TakenParameter,
                    None => Cause::Binding,
                };
                provenance.facts.push(Fact::new(owner, span, &[&name]));
            }
            // The final cause and site retain the existing cascade identity.
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
                .map(|root| root_key(self.own.definition.path(), ("locality", root))),
            ..Explanation::default()
        };
        if let (Some(cause), Some(index)) = (explanation.cause().cloned(), provenance.binding) {
            let shared = self.properties[self.body.bindings[index].ty].shared;
            let x = cause.names.first().cloned().unwrap_or_default();
            let repair_cause = if cause.kind == Cause::Capture && self.graph.scoped {
                Cause::BorrowedCapture
            } else {
                cause.kind
            };
            let (repairs, cautions) = locality_repairs(repair_cause, required.kind, &x, shared);
            explanation.repairs = repairs;
            if repair_cause == Cause::BorrowedCapture {
                let receiver =
                    matches!(required.kind, Requirement::Owned | Requirement::OwnedResult)
                        .then(|| self.capture_global_receiver(value))
                        .flatten();
                let needs_global =
                    !matches!(required.kind, Requirement::Owned | Requirement::OwnedResult)
                        || receiver.is_some();
                if needs_global
                    && self.independent_capture_copy(value, self.body.bindings[index].ty)
                {
                    for repair in &mut explanation.repairs {
                        if matches!(repair, Repair::KeepCaptureLocal(_)) {
                            *repair = Repair::DeepCopy(x.clone());
                        }
                    }
                }
                explanation.also_required.extend(receiver);
            }
            explanation.cautions = cautions;
        }
        explanation.required = Some(required);
        Pending {
            explanation,
            temporary: provenance.temporary,
            moved: None,
            installed: provenance.installed,
            uniqueness: None,
        }
    }
    /// Diagnostic-only evidence, deliberately limited to concrete counted
    /// sequences for captures and loop inputs. Generic, resource and callable
    /// values need conditional help.
    fn independent_capture_copy(&self, value: &Value, ty: TypeId) -> bool {
        if value.unplaced()
            || value.content.summary().scopes().next().is_some()
            || self.arenas.placed(&value.modes.relation)
            || !self.properties[ty].copyable
            || self.properties[ty].plain
        {
            return false;
        }
        let mut pending = vec![ty];
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let row = self.t.get(ty).unwrap();
            if self.arenas.placed(&row.modes.relation) {
                return false;
            }
            match &row.kind {
                TypeKind::Primitive(_) => {}
                TypeKind::Builtin {
                    kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline,
                    ..
                } => pending.extend(row.kind.value_parts()),
                _ => return false,
            }
        }
        true
    }
    /// OWN-1 precedes required modes. Recover the already checked receiving
    /// row from its call node, without running a second ownership judgment.
    fn capture_global_receiver(&self, value: &Value) -> Option<Fact<Requirement>> {
        let site = self.span_of(value.site?);
        let Op::Use { operation, .. } = &self.graph.nodes[self.node].op else {
            return None;
        };
        self.graph.nodes.iter().find_map(|node| {
            let Op::Call {
                expression,
                callee,
                arguments,
                requirements,
                ..
            } = &node.op
            else {
                return None;
            };
            if expression != operation {
                return None;
            }
            arguments
                .iter()
                .zip(requirements)
                .find_map(|(arg, required)| {
                    let span = self.span_of(arg.value);
                    let required = required.as_ref()?;
                    if span.start <= site.start
                        && span.end >= site.end
                        && self.arenas.global(&required.modes.relation)
                    {
                        let name = self.spelled(*callee)?;
                        Some(Fact::new(
                            Requirement::GlobalArgument,
                            self.span_of(*callee),
                            &[&name],
                        ))
                    } else {
                        None
                    }
                })
        })
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
                label: None,
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
            root: content.scopes().next().map(|(_, scope, _)| {
                root_key(self.own.definition.path(), ("locality", (2u8, scope)))
            }),
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
    /// CLO-4 remains a construction failure. Snapshot guidance changes what
    /// the user captures; it grants no mutable, ownership or authority promise.
    pub(super) fn mutable_capture_explanation(
        &self,
        index: usize,
        value: &Value,
        read_only: bool,
        required: Fact<Requirement>,
    ) -> Pending {
        // Owning selection says Copy even for writes in the body. H's
        // access rows describe explicit operations; assignments retain their
        // target separately. Both must be read-only, including nested uses.
        let within = |at: Span| at.start >= required.span.start && at.end <= required.span.end;
        let is_capture = |source: &SourcePlace| {
            matches!(&source.root, SourceRoot::Binding { owner, binding }
                if self.source_rows.binding(self.body, Some(*owner), binding) == Some(index))
        };
        let read_only = read_only
            && !self.body.places.iter().any(|source| {
                within(self.span_of(source.expression))
                    && matches!(
                        source.access,
                        SourceAccess::Mut | SourceAccess::Take | SourceAccess::Unique
                    )
                    && is_capture(source)
            })
            && !self.tree.statements.iter().any(|statement| {
                let hirsel_syntax::StmtKind::Assign { place, .. } = statement.kind else {
                    return false;
                };
                within(statement.origin.span)
                    && self
                        .source_rows
                        .source_place(place)
                        .is_some_and(|row| is_capture(&self.body.places[row]))
            });
        let mut pending = self.binding_explanation(index, Some(Cause::MutableCapture), required);
        let (source, _) = self.binding_site(index);
        let ty = self.body.bindings[index].ty;
        let plain = self.properties[ty].plain;
        let noalloc = self
            .graph_callable()
            .is_some_and(|(f, _)| f.allocation == hirsel_types::Allocation::NoAlloc);
        let deep_copy = !noalloc && self.independent_capture_copy(value, ty);
        // Counted snapshots must establish independence, even when the source
        // is currently unique: copying into an immutable binding can keep a
        // loan on its mutable owner. Do not propose a take of live storage.
        let saved = (read_only && (plain || deep_copy))
            .then(|| self.snapshot_name())
            .flatten();
        let repair = if let Some(saved) = saved {
            Repair::CaptureSnapshot {
                source,
                saved,
                deep_copy: !plain,
            }
        } else {
            let evidence = format!(
                "copyability: {}; locality: {}; current uniqueness: {}; allocation: {}; capture: {}",
                if self.properties[ty].copyable {
                    "copyable"
                } else {
                    "not copyable"
                },
                if self.arenas.global(&value.modes.relation)
                    && value.aliased().is_none()
                    && !value.unplaced()
                    && value.content.summary().scopes().next().is_none()
                {
                    "global content"
                } else {
                    "source or placement bounds remain"
                },
                if value.modes.uniqueness == Uniqueness::Unique {
                    "unique"
                } else {
                    "not proved unique"
                },
                if noalloc {
                    "@noalloc"
                } else {
                    "permitted, but independence still needs proof"
                },
                if read_only {
                    "read-only"
                } else {
                    "takes or mutates its source"
                },
            );
            Repair::MutableCaptureLocal { source, evidence }
        };
        pending.explanation.repairs.push(repair);
        pending
    }

    /// Avoid all written identifiers (including later closure locals) and
    /// available declarations/imports. This runs only for failed diagnostics.
    fn snapshot_name(&self) -> Option<String> {
        let used: BTreeSet<_> = self
            .tree
            .tokens
            .iter()
            .filter_map(|token| match token.kind {
                hirsel_syntax::TokenKind::Ident(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        (1..=64)
            .map(|suffix| {
                if suffix == 1 {
                    "saved".to_owned()
                } else {
                    format!("saved_{suffix}")
                }
            })
            .find(|name| {
                !used.contains(name.as_str())
                    && self
                        .signatures
                        .diagnostic_name_available(self.own.definition.path(), name)
            })
    }

    /// OWN-8 repairs consume the whole binding, never just the selected place.
    /// Use only retained declaration evidence; failing diagnostics must not
    /// invent a constructor for hidden, generic or indirect storage.
    pub(super) fn part_explanation(
        &self,
        state: &State,
        place: &Place,
        value: &Value,
        site: Span,
    ) -> Pending {
        let mut pending = self.binding_explanation(
            place.binding,
            Some(Cause::Part),
            Fact::new(Requirement::Owned, site, &[]),
        );
        let (owner, _) = self.binding_site(place.binding);
        let recipe = || {
            if value.element_place.is_some()
                || value.aliased().is_some()
                || state.slots[place.binding].value.aliased().is_some()
                || state.slots[place.binding].available != LIVE
                || self.finalizer_self(place.binding)
                || self.loaned(
                    state,
                    &Place {
                        binding: place.binding,
                        projections: vec![],
                    },
                )
                || !matches!(place.projections.as_slice(), [Projection::Field(_)])
            {
                return None;
            }
            let binding = &self.body.bindings[place.binding];
            if binding.owner != self.graph.owner
                || matches!(
                    binding_mode(self, place.binding),
                    Some(PassingMode::Borrow | PassingMode::Mut)
                )
            {
                return None;
            }
            let TypeKind::Named {
                definition,
                arguments,
            } = &self.t.get(binding.ty)?.kind
            else {
                return None;
            };
            let path = definition.path();
            let here = &self.syntax.item.definition;
            // External/private representations and imported aliases need the
            // author's checked API; do not guess a source-level import path.
            if path.0.package != here.0.package || path.0.modules != here.0.modules {
                return None;
            }
            let signature = self.signatures.cached(*definition)?;
            let SignatureKind::Type(declaration) = &signature.kind else {
                return None;
            };
            if declaration.resource || declaration.drop.is_some() {
                return Some(Repair::ResourcePart(owner.clone()));
            }
            if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
                || !arguments.is_empty()
                || !declaration.generics.is_empty()
                || declaration.distinct
            {
                return None;
            }
            let TypeKind::Record(fields) = &self.t.get(declaration.representation?)?.kind else {
                return None;
            };
            let [Projection::Field(requested)] = place.projections.as_slice() else {
                return None;
            };
            fields.get(*requested as usize)?;
            let mut used: BTreeSet<String> = self
                .tree
                .tokens
                .iter()
                .filter_map(|token| match token.kind {
                    hirsel_syntax::TokenKind::Ident(name) => Some(name.as_str().to_owned()),
                    _ => None,
                })
                .collect();
            used.extend(
                self.body
                    .resolution
                    .dependencies
                    .iter()
                    .map(|p| p.0.name.clone()),
            );
            let mut parts = Vec::new();
            for field in fields.iter() {
                let base = format!("moved_{}", field.name.as_str());
                let mut fresh = base.clone();
                let mut suffix = 2;
                while used.contains(&fresh)
                    || !self.signatures.diagnostic_name_available(here, &fresh)
                {
                    fresh = format!("{base}_{suffix}");
                    suffix += 1;
                    if suffix > 64 {
                        return None;
                    }
                }
                used.insert(fresh.clone());
                parts.push(format!("{}: {fresh}", field.name.as_str()));
            }
            Some(Repair::WholeRecord {
                owner: owner.clone(),
                statement: format!("let {}({}) = take {owner}", path.0.name, parts.join(", ")),
            })
        };
        pending
            .explanation
            .repairs
            .push(recipe().unwrap_or(Repair::WholeOwnerOnly(owner)));
        pending
    }
    /// A use of a moved binding: the move site comes from the trace.
    pub(super) fn moved(&self, index: usize, read: ExprId, required: Fact<Requirement>) -> Pending {
        let (name, _) = self.binding_site(index);
        let mut pending: Pending = Explanation {
            required: Some(required),
            root: Some(root_key(self.own.definition.path(), ("moved", index))),
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
        rule: RuleId,
        site: Span,
        message: &str,
        explain: impl FnOnce(&Self) -> Pending,
    ) -> Option<&mut Diagnostic> {
        if !self.emit {
            return None;
        }
        let mut pending = explain(self);
        pending.explanation.name_values(self.sites());
        self.diagnostics
            .push(Diagnostic::error(rule, site, message));
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
            .any(|(_, p)| p.moved.is_some() || p.installed.is_some() || p.uniqueness.is_some())
    }
    /// Recover trace sites for this graph's explanations, then render them.
    pub(super) fn explain(&mut self, initial: State, live: &LiveSets) {
        if self.explanations.is_empty() {
            return;
        }
        if self.needs_trace() {
            let explanations = std::mem::take(&mut self.explanations);
            let bindings: BTreeSet<_> = explanations
                .iter()
                .filter_map(|(_, p)| p.uniqueness.map(|(b, _)| b))
                .collect();
            self.emit = false;
            let inputs = solve(self, initial, live);
            self.trace = Some(Trace::default());
            for (id, input) in inputs.into_iter().enumerate() {
                if let Some(mut input) = input {
                    self.node = id;
                    let graph = self.graph;
                    let before = (!bindings.is_empty()).then(|| input.clone());
                    self.transfer(&graph.nodes[id].op, &mut input);
                    if let Some(before) = before {
                        for &binding in &bindings {
                            let step =
                                self.uniqueness_step(binding, &graph.nodes[id].op, &before, &input);
                            self.trace
                                .as_mut()
                                .unwrap()
                                .uniqueness
                                .insert((binding, id), step);
                        }
                    }
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
            if let Some((binding, node)) = pending.uniqueness {
                let (facts, repairs, related_result) =
                    self.uniqueness_causes(&trace, binding, node);
                pending.explanation.provided = facts;
                pending.explanation.repairs.extend(repairs);
                let (name, _) = self.binding_site(binding);
                let noalloc = self
                    .graph_callable()
                    .is_some_and(|(f, _)| f.allocation == hirsel_types::Allocation::NoAlloc);
                let binding = &self.body.bindings[binding];
                let copyable = self.properties[binding.ty].copyable;
                if related_result {
                    self.diagnostics[index].notes.push(
                        if !noalloc && copyable {
                            format!("Put confined result holders and their derived copies in a scope that ends before this operation, or replace the root with `{name} = unique {name}` when its concrete copyable data, locality and allocation contract permit it. Copying an unrelated result does not repair this root.")
                        } else {
                            "Put confined result holders and their derived copies in a scope that ends before this operation. Copying an unrelated result does not repair this root.".into()
                        }
                    );
                } else {
                    self.diagnostics[index].notes.push(
                        "Ending or dropping an ordinary counted alias does not restore this binding's uniqueness proof. Temporary read loans end separately; a plain `mut` handback needs its own checked preservation contract.".into()
                    );
                }
                self.diagnostics[index].notes.push(
                    "help: Supply an independent unique owner, or transfer an owned value with `take` before sharing when it is already proved unique. Changing a supplier to a `take` unique parameter also requires updating its callers to transfer genuinely unique owners; taking an already shared value does not make it unique.".into()
                );
                if !noalloc && copyable && !related_result {
                    let edit = if binding.mutable {
                        format!("replace the root with `{name} = unique {name}`")
                    } else {
                        format!(
                            "pass `unique {name}` instead of sharing or taking the aliased value"
                        )
                    };
                    self.diagnostics[index].notes.push(format!(
                        "For concrete copyable data, {edit} only when its locality and allocation contract permit an independent deep copy (may allocate; costs O(size); preserves arena placement). Copying an unrelated result does not repair this root."
                    ));
                } else if noalloc || !copyable {
                    self.diagnostics[index].notes.push(if noalloc {
                        "Allocating deep copies are unavailable under `@noalloc`; use an already unique owner or repair the checked ownership contract."
                    } else {
                        "A deep-copy edit is not established for noncopyable generic or resource content; use an already unique owner or repair the checked ownership contract."
                    }.into());
                }
            }
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

impl Checker<'_, '_> {
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
            .unwrap_or_else(|| hirsel_modes::explain::prose("the callee"));
        let span = self.span_of(argument);
        let mut diagnostic = Diagnostic::error(
            const { RuleId::of("OWN-4") },
            span,
            "this `copy` is redundant",
        );
        diagnostic.severity = hirsel_diag::Severity::Warning;
        diagnostic.fix = Some(hirsel_diag::Fix {
            span,
            replacement: name.clone(),
            message: "pass the last use directly to the borrowed parameter".into(),
        });
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
impl Checker<'_, '_> {
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
            pending.extend(hirsel_modes::expression_children(self.tree, e));
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
        Op::MapError { value, .. } => (vec![*value], vec![]),
        Op::IterationLoan { source, .. } => (vec![*source], vec![]),
        Op::ArgumentLoan { argument, .. } => (vec![*argument], vec![]),
        Op::Bind { value, .. } | Op::Return { value, .. } | Op::Propagate { value, .. } => {
            (vec![*value], vec![])
        }
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
fn within_shallow(checker: &Checker<'_, '_>, e: ExprId, names: &BTreeSet<ExprId>) -> bool {
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

/// The declaration's passing mode, independently of the closure's borrowed use.
fn binding_mode(checker: &Checker<'_, '_>, index: usize) -> Option<PassingMode> {
    let binding = &checker.body.bindings[index];
    match binding.binding {
        ResolvedName::Parameter(p) => Some(checker.parameter_passing(&binding.owner, p)),
        _ => None,
    }
}
