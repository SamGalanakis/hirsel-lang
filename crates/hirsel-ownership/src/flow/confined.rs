//! Narrow MODE-2 release placement. Reuse the finite holder/liveness analysis;
//! this is not ordinary counted-copy last-use recovery. See the option B design
//! in hirsel-briefs/unique-restoration-design-18/DESIGN.md (G11).
use super::*;

impl Checker<'_, '_> {
    /// Concrete owned data only. Data alone could be an abstract `T: data`;
    /// walk representations too, rejecting handles and unknown payloads.
    fn confined_data(&self, ty: TypeId) -> bool {
        let mut pending = vec![ty];
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            // Conservative bound on polymorphic recursive representation growth.
            if seen.len() > 256 {
                return false;
            }
            let Some(row) = self.t.get(ty) else {
                return false;
            };
            if row.modes.multiplicity == Multiplicity::Once {
                return false;
            }
            match &row.kind {
                TypeKind::Primitive(_) => {}
                TypeKind::Record(_)
                | TypeKind::Variant(_)
                | TypeKind::Tuple(_)
                | TypeKind::Box(_) => pending.extend(row.kind.value_parts()),
                TypeKind::Builtin {
                    kind:
                        BuiltinType::List
                        | BuiltinType::Array
                        | BuiltinType::Map
                        | BuiltinType::SortedMap
                        | BuiltinType::Set
                        | BuiltinType::Inline
                        | BuiltinType::Packed
                        | BuiltinType::Simd,
                    ..
                } => {
                    pending.extend(row.kind.value_parts());
                }
                TypeKind::Named { .. } => {
                    let expanded = hirsel_type_analysis::representation::expand_representation(
                        self.t,
                        self.signatures.type_memo(),
                        ty,
                        &mut |_, definition, arguments| {
                            Ok(self
                                .signatures
                                .cached(definition)
                                .map(|s| (s, vec![AuthoritySummary::Unknown; arguments.len()])))
                        },
                        None,
                    );
                    let Ok(expanded) = expanded else { return false };
                    pending.extend(row.kind.value_parts());
                    pending.push(expanded);
                }
                _ => return false,
            }
        }
        true
    }

    /// No release at or after a join, branch or back edge. A later branch
    /// does not invalidate releases already proved in the straight-line prefix.
    pub(super) fn confined_prefix(&self) -> BTreeSet<usize> {
        let mut incoming = vec![0; self.graph.nodes.len()];
        for node in &self.graph.nodes {
            for next in &node.next {
                incoming[*next] += 1;
            }
        }
        let mut node = self.graph.entry;
        let mut prefix = BTreeSet::new();
        loop {
            if incoming[node] > 1 || !prefix.insert(node) {
                break;
            }
            match self.graph.nodes[node].next.as_slice() {
                [next] => node = *next,
                _ => break,
            }
        }
        prefix
    }

    pub(super) fn confined_candidates(&self) -> BTreeSet<usize> {
        let mut candidates = BTreeSet::new();
        for node in self.confined_nodes.iter().map(|id| &self.graph.nodes[*id]) {
            let Op::Bind { pattern, value, .. } = node.op else {
                continue;
            };
            let ExprKind::Call { callee, .. } = self.tree.expressions[value.0 as usize].kind else {
                continue;
            };
            if self.callee_definition(callee).is_none() {
                continue;
            }
            let bindings = super::super::cfg::binders(self.source_rows, &[pattern], self.tree);
            let [binding] = bindings.as_slice() else {
                continue;
            };
            let row = &self.body.bindings[*binding];
            let property = self.properties[row.ty];
            if !row.mutable
                && property.copyable
                && property.data
                && !property.plain
                && self.confined_data(row.ty)
            {
                candidates.insert(*binding);
            }
        }
        // An ordinary copy, capture, installation or take through a call is
        // outside B. Deep copies erase dependencies only for concrete data.
        for node in &self.graph.nodes {
            match &node.op {
                Op::Use {
                    expression,
                    context,
                    operation,
                } => {
                    if let Some(p) = place(self.body, *expression, self.source_rows) {
                        let detached = *context == Context::Unique
                            || matches!(context, Context::Read | Context::View)
                                && self.property(*operation).is_some_and(|p| p.plain);
                        if !p.projections.is_empty() || !detached {
                            candidates.remove(&p.binding);
                        }
                    }
                }
                Op::Capture { .. } | Op::LocalFunction(_) => return BTreeSet::new(),
                _ => {}
            }
        }
        candidates
    }

    pub(super) fn release_confined(&mut self, state: &mut State, statement: hirsel_syntax::StmtId) {
        if !self.confined_nodes.contains(&self.node) {
            return;
        }
        let live = self.live.at(self.node);
        for binding in self.confined.clone() {
            if state.slots[binding].available != LIVE || live.contains(&Holder::Binding(binding)) {
                continue;
            }
            let roots = self.reached_bindings(state, &state.slots[binding].value);
            if roots.is_empty() || !roots.iter().all(|r| state.slots[*r].value.borrow_only) {
                continue;
            }
            // All evaluated/parked dependent temporaries must have ended.
            if roots
                .iter()
                .any(|r| !state.slots[*r].value.parked.is_empty())
                || state.values.iter().any(|(e, v)| {
                    !self.property(*e).is_some_and(|p| p.plain)
                        && !self.reached_bindings(state, v).is_disjoint(&roots)
                })
            {
                continue;
            }
            state.slots[binding].available = DEAD;
            state.slots[binding].value.content = Content::default();
            state.slots[binding].value.set_alias(None);
            if self.emit {
                self.facts.confined_releases.push((statement, binding));
            }
        }
    }
}
