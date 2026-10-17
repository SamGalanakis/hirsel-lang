//! The control-flow graph over normal and failure edges, dominators, the loop
//! nesting forest's feedback edges and the worklist solver.
//!
//! Every analysis is a fixpoint: dominators by Cooper-Harvey-Kennedy
//! iteration, dataflow by a worklist over monotone transfers on finite
//! lattices. Neither has an iteration cutoff, and both converge on
//! irreducible graphs (loops with several entries).
use crate::ir::{BlockId, EdgeId};
use std::collections::BTreeSet;

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
    dom: hirsel_graph::Dominators,
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
        let adjacent: Vec<Vec<usize>> = succ
            .iter()
            .map(|outs| outs.iter().map(|out| out.target.index()).collect())
            .collect();
        let dom = hirsel_graph::Dominators::new(&adjacent, entry.index());
        let rpo: Vec<_> = dom
            .reverse_postorder()
            .iter()
            .map(|&b| BlockId(b as u32))
            .collect();
        let feedback_pairs = hirsel_graph::feedback_edges(&adjacent, entry.index());
        let pairs = &feedback_pairs;
        let feedback = succ
            .iter()
            .enumerate()
            .flat_map(|(b, outs)| {
                outs.iter().filter_map(move |out| {
                    pairs.contains(&(b, out.target.index())).then_some(out.edge)
                })
            })
            .collect();
        let mut order = vec![None; blocks];
        for (i, b) in rpo.iter().enumerate() {
            order[b.index()] = Some(i);
        }
        Self {
            entry,
            succ,
            preds,
            source,
            rpo,
            order,
            dom,
            feedback,
        }
    }

    pub fn reachable(&self, block: BlockId) -> bool {
        self.order[block.index()].is_some()
    }

    /// Reflexive dominance on reachable blocks.
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        self.dom.dominates(a.index(), b.index())
    }
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
                    match g
                        .dom
                        .immediate_dominator(x.index())
                        .map(|b| BlockId(b as u32))
                    {
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
