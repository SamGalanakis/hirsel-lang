//! Content keys for the nodes of a possibly cyclic graph: Merkle hashes for
//! acyclic nodes and, inside a strongly connected component, a rooted DFS
//! encoding of the component with de Bruijn back-references. Keys depend only
//! on each node's local content and its edges in successor order, never on
//! node identities, the order of `nodes` or the order of discovery.
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;

/// The local content of one node, length-framed so encodings never collide.
#[derive(Default)]
pub struct KeyWriter {
    bytes: Vec<u8>,
}
impl KeyWriter {
    /// Appends one framed byte string.
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend((bytes.len() as u64).to_le_bytes());
        self.bytes.extend(bytes);
    }
    fn tag(&mut self, tag: u8) {
        self.bytes.push(tag);
    }
    fn number(&mut self, n: usize) {
        self.bytes.extend((n as u64).to_le_bytes());
    }
}

const NODE: u8 = 0;
const OUTSIDE: u8 = 1;
const BACK: u8 = 2;
const END: u8 = 3;

/// The key of every node reachable from `nodes` through `succ`. An acyclic
/// node's key hashes its local content and its successors' keys in order. A
/// member `m` of a cyclic component (a self-loop included) hashes the DFS
/// from `m` over the component: each node's local content, then each
/// successor as the key of a node outside the component, the discovery
/// ordinal of one already reached, or its own encoding when first reached.
/// Rooting at every member means no member order is chosen.
pub fn scc_keys<N: Copy + Eq + Hash>(
    nodes: &[N],
    succ: impl Fn(N) -> Vec<N>,
    local: impl Fn(N, &mut KeyWriter),
) -> HashMap<N, ContentHash> {
    let mut index = HashMap::new();
    let mut order = vec![];
    let mut adjacency: Vec<Vec<usize>> = vec![];
    let mut pending: Vec<N> = nodes.to_vec();
    pending.reverse();
    let mut intern = |n: N, order: &mut Vec<N>, pending: &mut Vec<N>| -> usize {
        *index.entry(n).or_insert_with(|| {
            order.push(n);
            pending.push(n);
            order.len() - 1
        })
    };
    for n in nodes {
        intern(*n, &mut order, &mut vec![]);
    }
    let mut next = 0;
    while next < order.len() {
        let targets = succ(order[next])
            .into_iter()
            .map(|t| intern(t, &mut order, &mut pending))
            .collect();
        adjacency.push(targets);
        next += 1;
    }
    let mut keys: Vec<Option<ContentHash>> = vec![None; order.len()];
    // Tarjan completes a component after every component it reaches.
    for component in crate::components(&adjacency) {
        let cyclic = component.len() > 1 || adjacency[component[0]].contains(&component[0]);
        if !cyclic {
            let node = component[0];
            let mut writer = KeyWriter::default();
            local(order[node], &mut writer);
            for target in &adjacency[node] {
                writer.bytes(&keys[*target].expect("successor keyed first").0);
            }
            keys[node] = Some(ContentHash::of_parts(
                "hirsel.scc.acyclic.v1",
                [&writer.bytes[..]],
            ));
            continue;
        }
        let members: BTreeSet<usize> = component.iter().copied().collect();
        let mut encoded = vec![];
        for &root in &component {
            let mut writer = KeyWriter::default();
            let mut discovered = BTreeMap::from([(root, 0usize)]);
            writer.tag(NODE);
            local(order[root], &mut writer);
            let mut frames = vec![(root, 0usize)];
            while let Some((node, cursor)) = frames.last_mut() {
                let Some(&target) = adjacency[*node].get(*cursor) else {
                    writer.tag(END);
                    frames.pop();
                    continue;
                };
                *cursor += 1;
                if !members.contains(&target) {
                    writer.tag(OUTSIDE);
                    writer.bytes(&keys[target].expect("outside component keyed first").0);
                } else if let Some(ordinal) = discovered.get(&target) {
                    writer.tag(BACK);
                    writer.number(*ordinal);
                } else {
                    discovered.insert(target, discovered.len());
                    writer.tag(NODE);
                    local(order[target], &mut writer);
                    frames.push((target, 0));
                }
            }
            encoded.push((
                root,
                ContentHash::of_parts("hirsel.scc.cyclic.v1", [&writer.bytes[..]]),
            ));
        }
        for (root, key) in encoded {
            keys[root] = Some(key);
        }
    }
    order
        .into_iter()
        .zip(keys)
        .map(|(n, k)| (n, k.expect("every component keyed")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(nodes: &[usize], succ: &[Vec<usize>], label: &[&str]) -> BTreeMap<usize, ContentHash> {
        scc_keys(
            nodes,
            |n| succ[n].clone(),
            |n, w| w.bytes(label[n].as_bytes()),
        )
        .into_iter()
        .collect()
    }

    #[test]
    fn keys_ignore_node_identity_and_request_order() {
        // Cons -> box Cons; Even <-> Odd; Pair -> (Even, leaf).
        let succ = vec![vec![0], vec![2], vec![1], vec![1, 4], vec![]];
        let label = ["cons", "even", "odd", "pair", "leaf"];
        let a = key(&[0, 1, 2, 3, 4], &succ, &label);
        let b = key(&[4, 3, 2, 1, 0], &succ, &label);
        assert_eq!(a, b);
        // The same graph under renumbered nodes.
        let permutation = [3, 0, 4, 2, 1];
        let mut renumbered = vec![vec![]; 5];
        let mut relabel = [""; 5];
        for (old, targets) in succ.iter().enumerate() {
            renumbered[permutation[old]] = targets.iter().map(|t| permutation[*t]).collect();
            relabel[permutation[old]] = label[old];
        }
        let c = key(&[0, 1, 2, 3, 4], &renumbered, &relabel);
        for old in 0..5 {
            assert_eq!(a[&old], c[&permutation[old]], "{}", label[old]);
        }
    }

    #[test]
    fn editing_a_member_changes_every_key_that_reaches_it() {
        let succ = vec![vec![1], vec![0, 2], vec![], vec![0]];
        let before = key(&[3], &succ, &["tree", "entry", "u64", "user"]);
        let after = key(&[3], &succ, &["tree", "entry2", "u64", "user"]);
        for n in [0, 1, 3] {
            assert_ne!(before[&n], after[&n], "{n}");
        }
        assert_eq!(before[&2], after[&2]);
    }

    #[test]
    fn members_with_equal_content_but_different_edges_differ() {
        // Even and Odd have the same local content; edge order tells them apart.
        let succ = vec![vec![1, 2], vec![0], vec![]];
        let keys = key(&[0], &succ, &["n", "n", "leaf"]);
        assert_ne!(keys[&0], keys[&1]);
        // A self-loop is a cycle, unlike the same node without the edge.
        let looped = key(&[0], &[vec![0]], &["n"]);
        let plain = key(&[0], &[vec![]], &["n"]);
        assert_ne!(looped[&0], plain[&0]);
    }

    #[test]
    fn symmetric_cycles_give_rotations_one_key() {
        // A -> B -> A with equal content: both rootings encode identically.
        let keys = key(&[0], &[vec![1], vec![0]], &["x", "x"]);
        assert_eq!(keys[&0], keys[&1]);
    }
}
