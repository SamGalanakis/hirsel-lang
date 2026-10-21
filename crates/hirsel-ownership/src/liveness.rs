//! Backward use-liveness of every value that can hold a loan (LANG-3).
//! A loan lasts until the last use of each holder on every path, not to the
//! end of the holder's scope. Holders are expression temporaries, bindings and
//! the cursor of a borrowed loop. Destroying a holder whose type has
//! user-visible destruction is a use on every exit path; writing a new value
//! into a holder is not a use of its old value.
use super::cfg::{Context, Graph, Op, binders, place};
use super::properties::PropertyTable;
use super::source_rows::SourceRows;
use super::*;
use hirsel_syntax::StmtKind;
use hirsel_syntax::SyntaxTree;
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Holder {
    Temporary(ExprId),
    Binding(usize),
    /// The cursor of a borrowed loop over this source, used at each advance.
    Iteration(ExprId),
}

pub(super) struct Liveness<'a> {
    pub t: &'a TypeInterner,
    pub tree: &'a SyntaxTree,
    pub body: &'a CheckedBody,
    pub rows: &'a SourceRows,
    pub properties: &'a PropertyTable,
    pub graph: &'a Graph,
    /// Temporaries that hold nothing and are never released (`constant_temporaries`).
    pub constants: &'a BTreeSet<ExprId>,
}
impl Liveness<'_> {
    /// The holders live after each node: some path from there reaches a use.
    pub fn after(&self) -> LiveSets {
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
        solve_liveness(&effects, graph.entry, |id| &graph.nodes[id].next)
    }
    /// A binding can hold a loan unless it is Plain (Plain values hold only
    /// identity loans, which PROV-1 tracks separately).
    fn holder(&self, binding: usize) -> Option<Holder> {
        (!self.properties[self.body.bindings[binding].ty].plain).then_some(Holder::Binding(binding))
    }
    /// Destroying a value that may contain a resource runs user code (DROP-2);
    /// copyable values have no user-visible destruction (DROP-1).
    fn destroyed(&self, binding: usize) -> bool {
        let ty = self.body.bindings[binding].ty;
        !self.properties[ty].copyable
            || self.t.get(ty).unwrap().modes.multiplicity == Multiplicity::Once
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
            Op::Return { value, .. } | Op::Propagate { value, .. } => {
                uses.push(Holder::Temporary(*value))
            }
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
        let held =
            |holder: &Holder| !matches!(holder, Holder::Temporary(e) if self.constants.contains(e));
        uses.retain(held);
        writes.retain(held);
        (uses, writes)
    }
}

/// Converged live sets with the original Holder order and graph-local domain.
pub(super) struct LiveSets {
    holders: Vec<Holder>,
    words: usize,
    count: usize,
    after: Vec<u64>,
}
#[derive(Clone, Copy)]
pub(super) struct LiveSet<'a> {
    holders: &'a [Holder],
    words: &'a [u64],
}
impl LiveSets {
    pub fn at(&self, node: usize) -> LiveSet<'_> {
        assert!(node < self.count);
        LiveSet {
            holders: &self.holders,
            words: &self.after[node * self.words..(node + 1) * self.words],
        }
    }
    #[cfg(test)]
    fn sets(&self) -> Vec<BTreeSet<Holder>> {
        (0..self.count)
            .map(|id| self.at(id).iter().copied().collect())
            .collect()
    }
}
impl<'a> LiveSet<'a> {
    pub fn contains(self, holder: &Holder) -> bool {
        self.holders
            .binary_search(holder)
            .is_ok_and(|ordinal| self.words[ordinal / 64] & (1 << (ordinal % 64)) != 0)
    }
    pub fn iter(self) -> impl Iterator<Item = &'a Holder> {
        self.words
            .iter()
            .enumerate()
            .flat_map(move |(index, &word)| {
                let mut word = word;
                std::iter::from_fn(move || {
                    if word == 0 {
                        return None;
                    }
                    let ordinal = index * 64 + word.trailing_zeros() as usize;
                    word &= word - 1;
                    Some(&self.holders[ordinal])
                })
            })
    }
}

fn solve_liveness<'a>(
    effects: &[(Vec<Holder>, Vec<Holder>)],
    entry: usize,
    successors: impl Fn(usize) -> &'a [usize],
) -> LiveSets {
    // Rustc15c1044 rustc_index/bit_set.rs:79-100,320-329,427-440:
    // union/subtract dense words and enumerate set bits in ordinal order.
    // Only this graph's holders enter the domain; global IDs are never indexes.
    let mut holders: Vec<_> = effects
        .iter()
        .flat_map(|(uses, writes)| uses.iter().chain(writes).copied())
        .collect();
    holders.sort_unstable();
    holders.dedup();
    let words = holders.len().div_ceil(64);
    let cells = effects.len() * words;
    let mut generated = vec![0u64; cells];
    let mut killed = vec![0u64; cells];
    for (id, (uses, writes)) in effects.iter().enumerate() {
        for (set, values) in [(&mut generated, uses), (&mut killed, writes)] {
            for holder in values {
                let ordinal = holders.binary_search(holder).unwrap();
                set[id * words + ordinal / 64] |= 1 << (ordinal % 64);
            }
        }
    }
    let mut before = vec![0u64; cells];
    let mut after = vec![0u64; cells];
    let mut predecessors = vec![vec![]; effects.len()];
    for id in 0..effects.len() {
        for next in successors(id) {
            predecessors[*next].push(id);
        }
    }
    let mut pending: VecDeque<_> = graph_postorder(effects.len(), entry, &successors).into();
    let mut queued = vec![true; effects.len()];
    let mut live = vec![0u64; words];
    while let Some(id) = pending.pop_front() {
        queued[id] = false;
        work::count(Counter::OwnershipLivenessSteps);
        live.fill(0);
        for next in successors(id) {
            for (word, incoming) in live
                .iter_mut()
                .zip(&before[*next * words..(*next + 1) * words])
            {
                *word |= incoming;
            }
        }
        work::count_by(
            Counter::OwnershipStateRows,
            live.iter().map(|word| u64::from(word.count_ones())).sum(),
        );
        let range = id * words..(id + 1) * words;
        after[range.clone()].copy_from_slice(&live);
        for ((word, generated), killed) in live
            .iter_mut()
            .zip(&generated[range.clone()])
            .zip(&killed[range.clone()])
        {
            *word = (*word & !killed) | generated;
        }
        if before[range.clone()] != live {
            before[range].copy_from_slice(&live);
            for previous in &predecessors[id] {
                if !queued[*previous] {
                    queued[*previous] = true;
                    pending.push_back(*previous);
                }
            }
        }
    }
    LiveSets {
        holders,
        words,
        count: effects.len(),
        after,
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

#[cfg(test)]
mod solver_tests {
    use super::*;

    // Independent whole-graph sweeps, without the production worklist or DFS.
    fn reference(
        effects: &[(Vec<Holder>, Vec<Holder>)],
        next: &[Vec<usize>],
    ) -> Vec<BTreeSet<Holder>> {
        let mut before = vec![BTreeSet::new(); next.len()];
        for _ in 0..=next.len() {
            let mut changed = false;
            for id in 0..next.len() {
                let mut incoming: BTreeSet<_> = next[id]
                    .iter()
                    .flat_map(|n| before[*n].iter().copied())
                    .collect();
                for write in &effects[id].1 {
                    incoming.remove(write);
                }
                incoming.extend(effects[id].0.iter().copied());
                changed |= incoming != before[id];
                before[id] = incoming;
            }
            if !changed {
                return next
                    .iter()
                    .map(|edges| {
                        edges
                            .iter()
                            .flat_map(|n| before[*n].iter().copied())
                            .collect()
                    })
                    .collect();
            }
        }
        panic!("finite liveness equations did not converge");
    }

    #[test]
    fn liveness_matches_set_equations_on_cycles_branches_and_sparse_holders() {
        for seed in 1..=32u64 {
            let mut random = seed;
            let mut draw = || {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random as usize
            };
            for holders in [0, 1, 63, 64, 65, 129] {
                let count = 1 + draw() % 17;
                let universe: Vec<_> = (0..holders)
                    .map(|i| match i % 3 {
                        0 => Holder::Temporary(ExprId(u32::MAX - i as u32)),
                        1 => Holder::Binding(usize::MAX - i),
                        _ => Holder::Iteration(ExprId(u32::MAX - i as u32)),
                    })
                    .collect();
                let mut effects = vec![(vec![], vec![]); count];
                let mut next = vec![vec![]; count];
                for id in 0..count {
                    for _ in 0..draw() % 4 {
                        let target = draw() % count;
                        next[id].push(target);
                    }
                    for holder in &universe {
                        if draw() % 5 == 0 {
                            effects[id].0.push(*holder);
                        }
                        if draw() % 5 == 0 {
                            effects[id].1.push(*holder);
                        }
                    }
                }
                // Include the whole universe, covering multiword tails and read+write.
                effects[0].0.extend(universe.iter().copied());
                effects[0].1.extend(universe.iter().copied());
                let expected = reference(&effects, &next);
                for entry in [0, count - 1] {
                    let actual = solve_liveness(&effects, entry, |id| &next[id]);
                    for (id, expected_set) in expected.iter().enumerate() {
                        assert_eq!(
                            actual.at(id).iter().copied().collect::<Vec<_>>(),
                            expected_set.iter().copied().collect::<Vec<_>>()
                        );
                        for holder in &universe {
                            assert_eq!(
                                actual.at(id).contains(holder),
                                expected_set.contains(holder)
                            );
                        }
                    }
                    let actual = actual.sets();
                    assert_eq!(
                        actual, expected,
                        "seed {seed}, holders {holders}, entry {entry}"
                    );
                }
            }
        }
        // Unreachable components still receive facts; writes kill before uses win.
        let h = Holder::Binding(4);
        let effects = vec![(vec![], vec![]), (vec![h], vec![h]), (vec![], vec![])];
        let next = vec![vec![], vec![2], vec![1]];
        assert_eq!(
            solve_liveness(&effects, 0, |id| &next[id]).sets(),
            vec![BTreeSet::new(), BTreeSet::from([h]), BTreeSet::from([h])]
        );
    }
}
