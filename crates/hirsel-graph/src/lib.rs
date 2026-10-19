//! Iterative graph algorithms over dense node indices. Every target and root
//! must index `succ`. Adjacency order determines DFS order; duplicate edges are
//! allowed. Dominators and loop decomposition converge without a cutoff, even
//! on irreducible graphs. No walk uses the native call stack.

#[cfg(test)]
hirsel_guard::install!();

use std::collections::{BTreeMap, BTreeSet};
mod keys;
pub use keys::{KeyWriter, scc_keys};

/// Nodes reachable from `entry`, in reverse DFS postorder.
pub fn reverse_postorder(succ: &[Vec<usize>], entry: usize) -> Vec<usize> {
    let mut order = postorder(succ, [entry]);
    order.reverse();
    order
}

/// DFS postorder from the supplied roots, skipping nodes already visited.
/// Supply the entry followed by all node indices to include dead components.
pub fn postorder(succ: &[Vec<usize>], roots: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut seen = vec![false; succ.len()];
    let mut post = Vec::new();
    for root in roots {
        if std::mem::replace(&mut seen[root], true) {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        while let Some((b, next)) = stack.pop() {
            if let Some(&target) = succ[b].get(next) {
                stack.push((b, next + 1));
                if !std::mem::replace(&mut seen[target], true) {
                    stack.push((target, 0));
                }
            } else {
                post.push(b);
            }
        }
    }
    post
}

/// Reflexive reachability from one root, in O(nodes + edges) time and space.
pub fn reachable(succ: &[Vec<usize>], entry: usize) -> Vec<bool> {
    let mut seen = vec![false; succ.len()];
    for node in reverse_postorder(succ, entry) {
        seen[node] = true;
    }
    seen
}

/// Cooper–Harvey–Kennedy immediate dominators with DFS intervals for O(1)
/// dominance queries. Unreachable nodes have no dominator or DFS interval.
pub struct Dominators {
    rpo: Vec<usize>,
    order: Vec<Option<usize>>,
    idom: Vec<Option<usize>>,
    span: Vec<Option<(usize, usize)>>,
}
impl Dominators {
    pub fn new(succ: &[Vec<usize>], entry: usize) -> Self {
        let rpo = reverse_postorder(succ, entry);
        let mut order = vec![None; succ.len()];
        for (i, &b) in rpo.iter().enumerate() {
            order[b] = Some(i);
        }
        let preds = predecessors(succ);
        let mut idom = vec![None; succ.len()];
        idom[entry] = Some(entry);
        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter().skip(1) {
                let mut new: Option<usize> = None;
                for &p in &preds[b] {
                    if idom[p].is_none() {
                        continue;
                    }
                    new = Some(match new {
                        None => p,
                        Some(mut a) => {
                            let mut p = p;
                            while a != p {
                                if order[a] > order[p] {
                                    a = idom[a].expect("visited dominator");
                                } else {
                                    p = idom[p].expect("visited dominator");
                                }
                            }
                            a
                        }
                    });
                }
                if idom[b] != new {
                    idom[b] = new;
                    changed = true;
                }
            }
        }
        let mut children = vec![Vec::new(); succ.len()];
        for &b in rpo.iter().skip(1) {
            children[idom[b].expect("reachable node")].push(b);
        }
        let mut span = vec![None; succ.len()];
        let mut entered = vec![0; succ.len()];
        let mut clock = 0;
        let mut stack = vec![(entry, 0usize)];
        while let Some((b, next)) = stack.pop() {
            if next == 0 {
                entered[b] = clock;
                clock += 1;
            }
            if let Some(&child) = children[b].get(next) {
                stack.push((b, next + 1));
                stack.push((child, 0));
            } else {
                span[b] = Some((entered[b], clock));
                clock += 1;
            }
        }
        Self {
            rpo,
            order,
            idom,
            span,
        }
    }
    pub fn reverse_postorder(&self) -> &[usize] {
        &self.rpo
    }
    pub fn order(&self, node: usize) -> Option<usize> {
        self.order[node]
    }
    /// The entry is its own immediate dominator.
    pub fn immediate_dominator(&self, node: usize) -> Option<usize> {
        self.idom[node]
    }
    pub fn reachable(&self, node: usize) -> bool {
        self.order[node].is_some()
    }
    /// Reflexive on reachable nodes. Unreachable nodes dominate nothing and
    /// are dominated by nothing.
    pub fn dominates(&self, a: usize, b: usize) -> bool {
        match (self.span[a], self.span[b]) {
            (Some((a_in, a_out)), Some((b_in, b_out))) => a_in <= b_in && b_out <= a_out,
            _ => false,
        }
    }
}

fn predecessors(succ: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut preds = vec![Vec::new(); succ.len()];
    for (source, targets) in succ.iter().enumerate() {
        for &target in targets {
            preds[target].push(source);
        }
    }
    preds
}

/// Tarjan SCCs of the whole graph. Members are sorted; component order follows
/// DFS completion, with node roots visited in index order.
pub fn components(succ: &[Vec<usize>]) -> Vec<Vec<usize>> {
    components_of(&(0..succ.len()).collect::<Vec<_>>(), |b| succ[b].clone())
}

// The same iterative Tarjan walk also handles induced subgraphs during loop
// decomposition. `next` must stay inside `nodes`.
fn components_of(nodes: &[usize], next: impl Fn(usize) -> Vec<usize>) -> Vec<Vec<usize>> {
    let mut index: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut on_stack = BTreeSet::new();
    let mut stack = Vec::new();
    let mut result = Vec::new();
    let mut counter = 0;
    for &root in nodes {
        if index.contains_key(&root) {
            continue;
        }
        let mut frames = vec![(root, next(root), 0usize)];
        index.insert(root, (counter, counter));
        counter += 1;
        stack.push(root);
        on_stack.insert(root);
        while let Some((node, targets, cursor)) = frames.last_mut() {
            let node = *node;
            if let Some(&target) = targets.get(*cursor) {
                *cursor += 1;
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
                    frames.push((target, next(target), 0));
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

/// Loop-forest feedback edges, identified by (source, target); every parallel
/// edge for a returned pair is feedback. Only entry-reachable cycles count.
/// In each cyclic SCC, remove its internal edges into its entries, then
/// decompose the remainder again. Entries include the root and nodes with an
/// incoming edge from outside the component in the original graph. With no
/// external entry all members are entries. The set breaks every cycle and is
/// independent of adjacency order, including for irreducible graphs.
pub fn feedback_edges(succ: &[Vec<usize>], entry: usize) -> BTreeSet<(usize, usize)> {
    let preds = predecessors(succ);
    let mut removed = BTreeSet::new();
    let mut work = vec![reverse_postorder(succ, entry)];
    while let Some(nodes) = work.pop() {
        let inside: BTreeSet<_> = nodes.iter().copied().collect();
        for component in components_of(&nodes, |b| {
            succ[b]
                .iter()
                .copied()
                .filter(|t| inside.contains(t) && !removed.contains(&(b, *t)))
                .collect()
        }) {
            let members: BTreeSet<_> = component.iter().copied().collect();
            let internal: Vec<_> = component
                .iter()
                .flat_map(|&b| succ[b].iter().map(move |&t| (b, t)))
                .filter(|&(b, t)| members.contains(&t) && !removed.contains(&(b, t)))
                .collect();
            if internal.is_empty() {
                continue;
            }
            let mut entries: BTreeSet<_> = component
                .iter()
                .copied()
                .filter(|&b| b == entry || preds[b].iter().any(|p| !members.contains(p)))
                .collect();
            if entries.is_empty() {
                entries = members;
            }
            for (b, t) in internal {
                if entries.contains(&t) {
                    removed.insert((b, t));
                }
            }
            work.push(component);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent transitive closure and vertex-deletion dominance reference.
    fn closure(succ: &[Vec<usize>], reflexive: bool) -> Vec<Vec<bool>> {
        let n = succ.len();
        let mut reach = vec![vec![false; n]; n];
        for (b, targets) in succ.iter().enumerate() {
            reach[b][b] = reflexive;
            for &t in targets {
                reach[b][t] = true;
            }
        }
        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    reach[i][j] |= reach[i][k] && reach[k][j];
                }
            }
        }
        reach
    }
    fn brute_components(succ: &[Vec<usize>], nodes: &BTreeSet<usize>) -> Vec<Vec<usize>> {
        let reach = closure(succ, true);
        let mut left = nodes.clone();
        let mut result = Vec::new();
        while let Some(b) = left.pop_first() {
            let mut component = vec![b];
            component.extend(left.iter().copied().filter(|&t| reach[b][t] && reach[t][b]));
            for t in &component {
                left.remove(t);
            }
            result.push(component);
        }
        result
    }
    fn brute_feedback(succ: &[Vec<usize>], entry: usize) -> BTreeSet<(usize, usize)> {
        let reach = closure(succ, true);
        let nodes: BTreeSet<usize> = (0..succ.len()).filter(|&b| reach[entry][b]).collect();
        let mut work = vec![nodes];
        let mut removed = BTreeSet::new();
        while let Some(nodes) = work.pop() {
            let sub: Vec<Vec<_>> = succ
                .iter()
                .enumerate()
                .map(|(b, ts)| {
                    ts.iter()
                        .copied()
                        .filter(|t| {
                            nodes.contains(&b) && nodes.contains(t) && !removed.contains(&(b, *t))
                        })
                        .collect()
                })
                .collect();
            for component in brute_components(&sub, &nodes) {
                let members: BTreeSet<_> = component.iter().copied().collect();
                let entries: BTreeSet<_> = members
                    .iter()
                    .copied()
                    .filter(|&b| {
                        b == entry
                            || succ
                                .iter()
                                .enumerate()
                                .any(|(p, ts)| !members.contains(&p) && ts.contains(&b))
                    })
                    .collect();
                let mut cyclic = false;
                for &b in &members {
                    for &t in &sub[b] {
                        if members.contains(&t) {
                            cyclic = true;
                            if entries.is_empty() || entries.contains(&t) {
                                removed.insert((b, t));
                            }
                        }
                    }
                }
                if cyclic {
                    work.push(members);
                }
            }
        }
        removed
    }
    fn check(succ: &[Vec<usize>], entry: usize) {
        let n = succ.len();
        let reach = closure(succ, true);
        let d = Dominators::new(succ, entry);
        assert_eq!(reachable(succ, entry), reach[entry]);
        let rpo = reverse_postorder(succ, entry);
        assert_eq!(d.reverse_postorder(), rpo);
        assert_eq!(rpo.first(), Some(&entry));
        assert_eq!(
            rpo.iter().copied().collect::<BTreeSet<_>>().len(),
            rpo.len()
        );
        let mut actual = components(succ);
        actual.sort();
        let mut expected = brute_components(succ, &(0..n).collect());
        expected.sort();
        assert_eq!(actual, expected, "{succ:?}");
        for a in 0..n {
            let without: Vec<Vec<_>> = succ
                .iter()
                .enumerate()
                .map(|(b, ts)| ts.iter().copied().filter(|&t| b != a && t != a).collect())
                .collect();
            let avoiding = closure(&without, true);
            for b in 0..n {
                let dominates = reach[entry][b] && (a == entry || a == b || !avoiding[entry][b]);
                assert_eq!(
                    d.dominates(a, b),
                    dominates,
                    "entry {entry}, {a} dominates {b}, {succ:?}"
                );
                if d.reachable(b) {
                    let mut ancestor = b;
                    let mut walked = a == b;
                    while let Some(parent) =
                        d.immediate_dominator(ancestor).filter(|&p| p != ancestor)
                    {
                        walked |= parent == a;
                        ancestor = parent;
                    }
                    assert_eq!(walked, dominates);
                } else {
                    assert_eq!(d.immediate_dominator(b), None);
                }
            }
        }
        let feedback = feedback_edges(succ, entry);
        assert_eq!(feedback, brute_feedback(succ, entry), "{succ:?}");
        let forward: Vec<Vec<_>> = succ
            .iter()
            .enumerate()
            .map(|(b, ts)| {
                ts.iter()
                    .copied()
                    .filter(|&t| !feedback.contains(&(b, t)))
                    .collect()
            })
            .collect();
        let remaining = closure(&forward, false);
        for b in 0..n {
            if reach[entry][b] {
                assert!(!remaining[b][b], "cycle at {b}: {succ:?}");
            }
        }
        let mut reversed = succ.to_vec();
        for ts in &mut reversed {
            ts.reverse();
        }
        assert_eq!(feedback_edges(&reversed, entry), feedback);
        // Pin the DFS ordering independently with a small recursive reference.
        fn visit(succ: &[Vec<usize>], b: usize, seen: &mut BTreeSet<usize>, post: &mut Vec<usize>) {
            if !seen.insert(b) {
                return;
            }
            for &t in &succ[b] {
                visit(succ, t, seen, post);
            }
            post.push(b);
        }
        let mut expected = Vec::new();
        visit(succ, entry, &mut BTreeSet::new(), &mut expected);
        expected.reverse();
        assert_eq!(rpo, expected);
        let mut expected = Vec::new();
        let mut seen = BTreeSet::new();
        for root in std::iter::once(entry).chain(0..n) {
            visit(succ, root, &mut seen, &mut expected);
        }
        assert_eq!(
            postorder(succ, std::iter::once(entry).chain(0..n)),
            expected
        );
    }
    #[test]
    fn random_graphs_agree_with_brute_force() {
        // Fixed seed makes this property suite reproducible on every build.
        let mut rng = 0x6a09_e667_f3bc_c909u64;
        for case in 0..1200 {
            let n = 1 + case % 14;
            let mut graph = vec![Vec::new(); n];
            for ts in &mut graph {
                for t in 0..n {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    if rng % 7 <= (case % 3) as u64 {
                        ts.push(t);
                    }
                    if rng.is_multiple_of(31) {
                        ts.push(t);
                    } // Parallel edges.
                }
            }
            check(&graph, case % n);
        }
    }
    #[test]
    fn nested_and_irreducible_loops_and_unreachable_predecessors() {
        assert!(components(&[]).is_empty());
        assert!(postorder(&[], []).is_empty());
        for graph in [
            vec![vec![1], vec![2, 3], vec![2, 1], vec![]],
            vec![vec![1, 2], vec![2, 3], vec![1, 3], vec![4], vec![]],
            vec![vec![1], vec![2], vec![1], vec![2], vec![4]],
        ] {
            check(&graph, 0);
        }
        assert_eq!(
            feedback_edges(&[vec![1, 2], vec![2], vec![1]], 0),
            BTreeSet::from([(1, 2), (2, 1)])
        );
        let mut graph = vec![vec![1], vec![2], vec![1], vec![2]];
        assert_eq!(feedback_edges(&graph, 0), BTreeSet::from([(1, 2), (2, 1)]));
        graph[3].clear();
        assert_eq!(feedback_edges(&graph, 0), BTreeSet::from([(2, 1)]));
    }
    #[test]
    fn deep_walks_use_explicit_stacks() {
        let n = 20_000;
        let graph: Vec<_> = (0..n)
            .map(|b| if b + 1 < n { vec![b + 1] } else { vec![] })
            .collect();
        let d = Dominators::new(&graph, 0);
        assert!(d.dominates(0, n - 1));
        assert!(!d.dominates(n - 1, 0));
        assert_eq!(components(&graph).len(), n);
        assert!(feedback_edges(&graph, 0).is_empty());
    }
}
