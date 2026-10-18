//! Backward use-liveness of every value that can hold a loan (LANG-3).
//! A loan lasts until the last use of each holder on every path, not to the
//! end of the holder's scope. Holders are expression temporaries, bindings and
//! the cursor of a borrowed loop. Destroying a holder whose type has
//! user-visible destruction is a use on every exit path; writing a new value
//! into a holder is not a use of its old value.
use super::cfg::{Context, Graph, Op, binders, place};
use super::properties::Properties;
use super::source_rows::SourceRows;
use super::*;
use hirsel_syntax::StmtKind;
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Holder {
    Temporary(ExprId),
    Binding(usize),
    /// The cursor of a borrowed loop over this source, used at each advance.
    Iteration(ExprId),
}

pub(super) struct Liveness<'a> {
    pub tree: &'a SyntaxTree,
    pub body: &'a CheckedBody,
    pub rows: &'a SourceRows,
    pub properties: &'a [Properties],
    pub graph: &'a Graph,
}
impl Liveness<'_> {
    /// The holders live after each node: some path from there reaches a use.
    pub fn after(&self) -> Vec<BTreeSet<Holder>> {
        let graph = self.graph;
        let iterations: BTreeSet<_> = graph
            .nodes
            .iter()
            .filter_map(|node| match node.op {
                Op::IterationLoan { source, .. } => Some(source),
                _ => None,
            })
            .collect();
        let effects: Vec<_> = graph
            .nodes
            .iter()
            .map(|node| self.effects(&node.op, &iterations))
            .collect();
        let mut before = vec![BTreeSet::new(); graph.nodes.len()];
        let mut after = before.clone();
        let mut predecessors = vec![vec![]; graph.nodes.len()];
        for (id, node) in graph.nodes.iter().enumerate() {
            for next in &node.next {
                predecessors[*next].push(id);
            }
        }
        let mut pending: VecDeque<_> =
            graph_postorder(graph.nodes.len(), graph.entry, |id| &graph.nodes[id].next).into();
        let mut queued = vec![true; graph.nodes.len()];
        while let Some(id) = pending.pop_front() {
            queued[id] = false;
            work::count(Counter::OwnershipLivenessSteps);
            let mut live = BTreeSet::new();
            for next in &graph.nodes[id].next {
                live.extend(&before[*next]);
            }
            after[id] = live.clone();
            let (uses, writes) = &effects[id];
            for write in writes {
                live.remove(write);
            }
            live.extend(uses);
            if before[id] != live {
                before[id] = live;
                for previous in &predecessors[id] {
                    if !queued[*previous] {
                        queued[*previous] = true;
                        pending.push_back(*previous);
                    }
                }
            }
        }
        after
    }
    /// A binding can hold a loan unless it is Plain (Plain values hold only
    /// identity loans, which PROV-1 tracks separately).
    fn holder(&self, binding: usize) -> Option<Holder> {
        (!self.properties[self.body.bindings[binding].ty.index()].plain)
            .then_some(Holder::Binding(binding))
    }
    /// Destroying a value that may contain a resource runs user code (DROP-2);
    /// copyable values have no user-visible destruction (DROP-1).
    fn destroyed(&self, binding: usize) -> bool {
        let ty = self.body.bindings[binding].ty;
        !self.properties[ty.index()].copyable
            || self.body.types.get(ty).unwrap().modes.multiplicity == Multiplicity::Once
    }
    fn captures(&self, closure: ExprId, uses: &mut Vec<Holder>) {
        let Some(index) = self.rows.closure(closure) else {
            return;
        };
        for (ordinal, capture) in self.body.closures[index].captures.iter().enumerate() {
            let owner = self
                .graph
                .capture_modes
                .get(closure, ordinal as u32)
                .map_or(self.graph.owner, |c| c.owner);
            let owner = (!matches!(capture.binding, ResolvedName::Local(_))).then_some(owner);
            if let Some(binding) = self.rows.binding(self.body, owner, &capture.binding) {
                uses.extend(self.holder(binding));
            }
        }
    }
    /// The holders an operation uses, and those it overwrites without use.
    fn effects(&self, op: &Op, iterations: &BTreeSet<ExprId>) -> (Vec<Holder>, Vec<Holder>) {
        let mut uses = vec![];
        let mut writes = vec![];
        match op {
            Op::Use {
                expression,
                context,
                ..
            } => {
                writes.push(Holder::Temporary(*expression));
                if let Some(place) = place(self.body, *expression, self.rows) {
                    // Addressing a place to write it is not a use, unless the
                    // write destroys displaced content with user-visible effects.
                    if *context != Context::Address || self.destroyed(place.binding) {
                        uses.extend(self.holder(place.binding));
                    }
                }
            }
            Op::Capture { expression, .. } => {
                writes.push(Holder::Temporary(*expression));
                self.captures(*expression, &mut uses);
            }
            Op::LocalFunction(statement) => {
                if let StmtKind::Function { body, .. } =
                    self.tree.statements[statement.0 as usize].kind
                {
                    self.captures(body, &mut uses);
                }
                for (index, binding) in self.body.bindings.iter().enumerate() {
                    if binding.site == NameSite::Statement(*statement)
                        && matches!(binding.binding, ResolvedName::Local(_))
                    {
                        writes.extend(self.holder(index));
                    }
                }
            }
            Op::Finish {
                expression,
                children,
                ..
            } => {
                writes.push(Holder::Temporary(*expression));
                uses.extend(children.iter().map(|c| Holder::Temporary(*c)));
            }
            Op::Forward { expression, child } => {
                writes.push(Holder::Temporary(*expression));
                uses.push(Holder::Temporary(*child));
            }
            Op::MapError { value, .. } => uses.push(Holder::Temporary(*value)),
            Op::IterationLoan { source, begin } => {
                uses.push(Holder::Temporary(*source));
                if *begin {
                    writes.push(Holder::Iteration(*source));
                }
            }
            Op::ArgumentLoan { argument, .. } => uses.push(Holder::Temporary(*argument)),
            Op::Bind { pattern, value, .. } => {
                uses.push(Holder::Temporary(*value));
                // Each advance of a borrowed loop uses its cursor.
                if iterations.contains(value) {
                    uses.push(Holder::Iteration(*value));
                }
                writes.extend(
                    binders(self.rows, &[*pattern], self.tree)
                        .into_iter()
                        .filter_map(|binding| self.holder(binding)),
                );
            }
            Op::Return { value, .. } => uses.push(Holder::Temporary(*value)),
            Op::Write {
                place: target,
                value,
                compound,
                ..
            } => {
                uses.push(Holder::Temporary(*value));
                if let Some(place) = place(self.body, *target, self.rows) {
                    if *compound || self.destroyed(place.binding) {
                        uses.extend(self.holder(place.binding));
                    } else if place.projections.is_empty() {
                        writes.extend(self.holder(place.binding));
                    }
                }
            }
            Op::Call {
                expression,
                callee,
                arguments,
                ..
            } => {
                writes.push(Holder::Temporary(*expression));
                uses.push(Holder::Temporary(*callee));
                uses.extend(arguments.iter().map(|a| Holder::Temporary(a.value)));
            }
            Op::Kill(bindings) => {
                for &binding in bindings {
                    if self.destroyed(binding) {
                        uses.extend(self.holder(binding));
                    } else {
                        writes.extend(self.holder(binding));
                    }
                }
            }
            Op::Nop | Op::ReturnUnit | Op::IdentityReady(_) | Op::StatementEnd(_) => {}
        }
        (uses, writes)
    }
}

/// Dependencies precede their readers in a backward walk. DFS is iterative,
/// and visits unreachable components too, preserving recovery/fact coverage.
pub(super) fn graph_postorder<'a>(
    count: usize,
    entry: usize,
    successors: impl Fn(usize) -> &'a [usize],
) -> Vec<usize> {
    let mut seen = vec![false; count];
    let mut order = Vec::with_capacity(count);
    for root in std::iter::once(entry).chain(0..count) {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut stack = vec![(root, 0)];
        while let Some((node, next)) = stack.last_mut() {
            if let Some(&child) = successors(*node).get(*next) {
                *next += 1;
                if !seen[child] {
                    seen[child] = true;
                    stack.push((child, 0));
                }
            } else {
                order.push(*node);
                stack.pop();
            }
        }
    }
    order
}
