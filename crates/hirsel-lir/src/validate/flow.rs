//! The control-flow graph over normal and failure edges, dominators, the loop
//! nesting forest's feedback edges and the worklist solver.
//!
//! Every analysis is a fixpoint: dominators by Cooper-Harvey-Kennedy
//! iteration, dataflow by a worklist over monotone transfers on finite
//! lattices. Neither has an iteration cutoff, and both converge on
//! irreducible graphs (loops with several entries).
use crate::ir::{BlockId, EdgeId};
use std::collections::{BTreeMap, BTreeSet};

/// Where an edge leaves its block: an instruction's failure edge, or the
/// terminator (including a call's or raise's failure edge).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Origin {
    Instruction(u32),
    Terminator,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Out {
    pub edge: EdgeId,
    pub target: BlockId,
    pub origin: Origin,
}

pub(super) struct Cfg {
    pub entry: BlockId,
    /// Per block, instruction failure edges in order, then terminator edges.
    pub succ: Vec<Vec<Out>>,
    /// Per block, incoming edges with their source block.
    pub preds: Vec<Vec<(BlockId, EdgeId)>>,
    /// Per edge, its source block and origin.
    pub source: Vec<Option<(BlockId, Origin)>>,
    /// Reachable blocks in reverse postorder.
    pub rpo: Vec<BlockId>,
    /// Reverse-postorder index of each reachable block.
    pub order: Vec<Option<usize>>,
    pub idom: Vec<Option<BlockId>>,
    /// Entry and exit times of each reachable block in a walk of the
    /// dominator tree: a block dominates exactly the blocks inside its span.
    span: Vec<Option<(u32, u32)>>,
    /// Edges the loop nesting forest removes to break every cycle.
    pub feedback: BTreeSet<EdgeId>,
}

impl Cfg {
    /// `outs` lists every edge use as (source block, edge, target, origin).
    pub fn new(blocks: usize, edges: usize, entry: BlockId, outs: &[(BlockId, Out)]) -> Self {
        let mut succ = vec![Vec::new(); blocks];
        let mut preds = vec![Vec::new(); blocks];
        let mut source = vec![None; edges];
        for &(block, out) in outs {
            succ[block.index()].push(out);
            preds[out.target.index()].push((block, out.edge));
            source[out.edge.index()] = Some((block, out.origin));
        }
        let rpo = reverse_postorder(&succ, entry);
        let mut order = vec![None; blocks];
        for (i, b) in rpo.iter().enumerate() {
            order[b.index()] = Some(i);
        }
        let mut cfg = Self {
            entry,
            succ,
            preds,
            source,
            rpo,
            order,
            idom: vec![None; blocks],
            span: vec![None; blocks],
            feedback: BTreeSet::new(),
        };
        cfg.idom = cfg.dominators();
        cfg.span = cfg.spans();
        cfg.feedback = cfg.forest_feedback();
        cfg
    }

    pub fn reachable(&self, block: BlockId) -> bool {
        self.order[block.index()].is_some()
    }

    /// Cooper, Harvey and Kennedy's iterative dominators, run to a fixpoint.
    fn dominators(&self) -> Vec<Option<BlockId>> {
        let mut idom: Vec<Option<BlockId>> = vec![None; self.succ.len()];
        idom[self.entry.index()] = Some(self.entry);
        let mut changed = true;
        while changed {
            changed = false;
            for &b in self.rpo.iter().skip(1) {
                let mut new = None;
                for &(p, _) in &self.preds[b.index()] {
                    if idom[p.index()].is_none() {
                        continue;
                    }
                    new = Some(match new {
                        None => p,
                        Some(current) => self.intersect(&idom, p, current),
                    });
                }
                if new.is_some() && idom[b.index()] != new {
                    idom[b.index()] = new;
                    changed = true;
                }
            }
        }
        idom
    }

    fn intersect(&self, idom: &[Option<BlockId>], a: BlockId, b: BlockId) -> BlockId {
        let rank = |x: BlockId| self.order[x.index()].unwrap_or(usize::MAX);
        let (mut a, mut b) = (a, b);
        while a != b {
            while rank(a) > rank(b) {
                a = idom[a.index()].unwrap_or(self.entry);
            }
            while rank(b) > rank(a) {
                b = idom[b.index()].unwrap_or(self.entry);
            }
        }
        a
    }

    /// Number the dominator tree: each block's entry and exit time.
    fn spans(&self) -> Vec<Option<(u32, u32)>> {
        let mut children = vec![Vec::new(); self.succ.len()];
        for &b in &self.rpo {
            if let Some(parent) = self.idom[b.index()].filter(|&p| p != b) {
                children[parent.index()].push(b);
            }
        }
        let mut span = vec![None; self.succ.len()];
        let mut clock = 0u32;
        let mut stack = vec![(self.entry, 0usize)];
        let mut entered = vec![0u32; self.succ.len()];
        while let Some((b, next)) = stack.pop() {
            if next == 0 {
                entered[b.index()] = clock;
                clock += 1;
            }
            if let Some(&child) = children[b.index()].get(next) {
                stack.push((b, next + 1));
                stack.push((child, 0));
            } else {
                span[b.index()] = Some((entered[b.index()], clock));
                clock += 1;
            }
        }
        span
    }

    /// Whether `a` dominates `b` (reflexive). Unreachable blocks dominate
    /// nothing and are dominated by nothing.
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        match (self.span[a.index()], self.span[b.index()]) {
            (Some((a_in, a_out)), Some((b_in, b_out))) => a_in <= b_in && b_out <= a_out,
            _ => false,
        }
    }

    /// The loop nesting forest's feedback edges (recursive strongly connected
    /// components): in each cyclic component, every edge into one of its
    /// entries is a feedback edge; removing them, the component's remainder is
    /// decomposed again. Every cycle, reducible or not, contains one, and the
    /// set does not depend on a search order.
    fn forest_feedback(&self) -> BTreeSet<EdgeId> {
        let mut removed = BTreeSet::new();
        let mut work = vec![self.rpo.clone()];
        while let Some(nodes) = work.pop() {
            let inside: BTreeSet<_> = nodes.iter().copied().collect();
            for component in components(&nodes, |b| {
                self.succ[b.index()]
                    .iter()
                    .filter(|o| inside.contains(&o.target) && !removed.contains(&o.edge))
                    .map(|o| o.target)
                    .collect()
            }) {
                let members: BTreeSet<_> = component.iter().copied().collect();
                let internal: Vec<_> = component
                    .iter()
                    .flat_map(|&b| self.succ[b.index()].iter())
                    .filter(|o| members.contains(&o.target) && !removed.contains(&o.edge))
                    .copied()
                    .collect();
                if internal.is_empty() {
                    continue;
                }
                let mut entries: BTreeSet<_> = component
                    .iter()
                    .copied()
                    .filter(|&b| {
                        b == self.entry
                            || self.preds[b.index()]
                                .iter()
                                .any(|(p, _)| !members.contains(p))
                    })
                    .collect();
                if entries.is_empty() {
                    entries = members.clone();
                }
                for out in internal {
                    if entries.contains(&out.target) {
                        removed.insert(out.edge);
                    }
                }
                work.push(component);
            }
        }
        removed
    }
}

fn reverse_postorder(succ: &[Vec<Out>], entry: BlockId) -> Vec<BlockId> {
    let mut seen = vec![false; succ.len()];
    let mut post = Vec::new();
    let mut stack = vec![(entry, 0usize)];
    seen[entry.index()] = true;
    while let Some((b, next)) = stack.pop() {
        if let Some(out) = succ[b.index()].get(next) {
            stack.push((b, next + 1));
            if !seen[out.target.index()] {
                seen[out.target.index()] = true;
                stack.push((out.target, 0));
            }
        } else {
            post.push(b);
        }
    }
    post.reverse();
    post
}

/// Tarjan's strongly connected components of the subgraph on `nodes`,
/// iteratively, so deep graphs do not exhaust the native stack.
fn components(nodes: &[BlockId], succ: impl Fn(BlockId) -> Vec<BlockId>) -> Vec<Vec<BlockId>> {
    let mut index: BTreeMap<BlockId, (usize, usize)> = BTreeMap::new();
    let mut on_stack = BTreeSet::new();
    let mut stack = Vec::new();
    let mut result = Vec::new();
    let mut counter = 0;
    for &root in nodes {
        if index.contains_key(&root) {
            continue;
        }
        let mut frames = vec![(root, succ(root), 0usize)];
        index.insert(root, (counter, counter));
        counter += 1;
        stack.push(root);
        on_stack.insert(root);
        while let Some((node, targets, next)) = frames.last_mut() {
            let node = *node;
            if let Some(&target) = targets.get(*next) {
                *next += 1;
                if let Some(&(target_index, _)) = index.get(&target) {
                    if on_stack.contains(&target) {
                        let low = &mut index.get_mut(&node).expect("visited").1;
                        *low = (*low).min(target_index);
                    }
                } else {
                    index.insert(target, (counter, counter));
                    counter += 1;
                    stack.push(target);
                    on_stack.insert(target);
                    frames.push((target, succ(target), 0));
                }
                continue;
            }
            frames.pop();
            let (node_index, node_low) = index[&node];
            if let Some((parent, _, _)) = frames.last() {
                let low = &mut index.get_mut(parent).expect("visited").1;
                *low = (*low).min(node_low);
            }
            if node_index == node_low {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack.remove(&member);
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort();
                result.push(component);
            }
        }
    }
    result
}

/// Forward worklist dataflow to a fixpoint. `transfer` maps a block's
/// in-state to one out-state per outgoing edge; a block's in-state is the join
/// of its predecessors' edge states (and `init` at the entry), computed when
/// the block is visited, so a block reached by many edges joins them once per
/// visit and not once per edge that changed. Blocks are visited in reverse
/// postorder, earliest first. Termination rests on monotone transfers over
/// finite lattices, never on a bound.
pub(super) fn solve<T: Clone + PartialEq>(
    cfg: &Cfg,
    init: T,
    mut transfer: impl FnMut(BlockId, &T) -> Vec<(EdgeId, T)>,
    join: impl Fn(&mut T, &T),
) -> Vec<Option<T>> {
    let mut ins: Vec<Option<T>> = vec![None; cfg.succ.len()];
    let mut outs: Vec<Option<T>> = vec![None; cfg.source.len()];
    let mut targets: Vec<Option<BlockId>> = vec![None; cfg.source.len()];
    for out in cfg.succ.iter().flatten() {
        targets[out.edge.index()] = Some(out.target);
    }
    let mut work = BTreeSet::from([0usize]);
    while let Some(next) = work.pop_first() {
        let block = cfg.rpo[next];
        let arriving = (block == cfg.entry)
            .then(|| init.clone())
            .into_iter()
            .chain(
                cfg.preds[block.index()]
                    .iter()
                    .filter_map(|&(_, e)| outs[e.index()].clone()),
            );
        let joined = join_all(arriving.collect(), &join);
        if joined == ins[block.index()] {
            continue;
        }
        ins[block.index()] = joined;
        let Some(state) = &ins[block.index()] else {
            continue;
        };
        for (edge, out) in transfer(block, state) {
            if outs[edge.index()].as_ref() == Some(&out) {
                continue;
            }
            outs[edge.index()] = Some(out);
            let target = targets[edge.index()].expect("an edge of this block");
            if let Some(order) = cfg.order[target.index()] {
                work.insert(order);
            }
        }
    }
    ins
}

/// The join of `states`, neighbours first, level by level. The states
/// reaching a block with many predecessors (a cleanup tail shared by many
/// failure sites) are successive points of one function and share structure
/// with their neighbours, so each level costs what differs within its pairs:
/// joining them one by one into an accumulator would instead cost, for each,
/// all that the accumulator has gathered.
fn join_all<T>(mut states: Vec<T>, join: impl Fn(&mut T, &T)) -> Option<T> {
    while states.len() > 1 {
        let mut pairs = states.into_iter();
        let mut next = Vec::with_capacity(pairs.len().div_ceil(2));
        while let Some(mut a) = pairs.next() {
            if let Some(b) = pairs.next() {
                join(&mut a, &b);
            }
            next.push(a);
        }
        states = next;
    }
    states.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(blocks: usize, edges: &[(u32, u32)]) -> Cfg {
        let outs: Vec<_> = edges
            .iter()
            .enumerate()
            .map(|(i, &(from, to))| {
                (
                    BlockId(from),
                    Out {
                        edge: EdgeId(i as u32),
                        target: BlockId(to),
                        origin: Origin::Terminator,
                    },
                )
            })
            .collect();
        Cfg::new(blocks, edges.len(), BlockId(0), &outs)
    }

    #[test]
    fn feedback_edges_break_reducible_nested_and_irreducible_cycles() {
        // 0 -> 1 -> 2 -> 1 (back), 2 -> 3.
        let g = cfg(4, &[(0, 1), (1, 2), (2, 1), (2, 3)]);
        assert_eq!(g.feedback, BTreeSet::from([EdgeId(2)]));
        // Nested: 0 -> 1 -> 2 -> 2 (inner self loop) -> 1 (outer back).
        let g = cfg(4, &[(0, 1), (1, 2), (2, 2), (2, 1), (1, 3)]);
        assert_eq!(g.feedback, BTreeSet::from([EdgeId(2), EdgeId(3)]));
        // Irreducible: 0 -> 1, 0 -> 2, 1 <-> 2; both entries are fed back.
        let g = cfg(4, &[(0, 1), (0, 2), (1, 2), (2, 1), (2, 3)]);
        assert_eq!(g.feedback, BTreeSet::from([EdgeId(2), EdgeId(3)]));
        // A forward edge to a join inside a loop is not a feedback edge.
        let g = cfg(5, &[(0, 1), (1, 2), (1, 3), (2, 3), (3, 1), (3, 4)]);
        assert_eq!(g.feedback, BTreeSet::from([EdgeId(4)]));
    }

    #[test]
    fn dominators_converge_on_a_loop_with_two_entries() {
        let g = cfg(5, &[(0, 1), (0, 2), (1, 2), (2, 1), (1, 3), (2, 3), (3, 4)]);
        // The span test agrees with walking the immediate-dominator chain.
        for a in 0..5 {
            for b in 0..5 {
                let mut x = BlockId(b);
                let walked = loop {
                    if x == BlockId(a) {
                        break true;
                    }
                    match g.idom[x.index()] {
                        Some(parent) if parent != x => x = parent,
                        _ => break false,
                    }
                };
                assert_eq!(g.dominates(BlockId(a), BlockId(b)), walked, "{a} {b}");
            }
        }
        assert!(g.dominates(BlockId(0), BlockId(3)));
        assert!(!g.dominates(BlockId(1), BlockId(2)));
        assert!(!g.dominates(BlockId(2), BlockId(1)));
        assert!(g.dominates(BlockId(3), BlockId(4)));
        // A may-reach set converges over the irreducible cycle.
        let ins = solve(
            &g,
            BTreeSet::new(),
            |b, s: &BTreeSet<u32>| {
                let mut s = s.clone();
                s.insert(b.0);
                g.succ[b.index()]
                    .iter()
                    .map(|o| (o.edge, s.clone()))
                    .collect()
            },
            |a, b| a.extend(b.iter().copied()),
        );
        assert_eq!(ins[3], Some(BTreeSet::from([0, 1, 2])));
        assert_eq!(ins[1], Some(BTreeSet::from([0, 1, 2])));
    }
}
