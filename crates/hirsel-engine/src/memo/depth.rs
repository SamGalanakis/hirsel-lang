//! Compile-time dependency depth as logical height (design §1c, rule 5).
//!
//! Every constant, generator and staged evaluation result stores
//! `DepthEvidence`: its height and an overflow table `first[m]`, the site of
//! the first request, in dependency-request order, that overflows a
//! cache-free depth-first evaluation whose own requests are made at stack
//! length m. A request is decided from the evidence alone, so a result that
//! was computed, came from the store or was owned by another worker gives the
//! verdict and site a cache-free serial evaluation gives.
use hirsel_span::Span;
use hirsel_syntax::LexicalOwner;

/// A request site: the requesting item and an item-relative span.
pub type SiteSpan = (LexicalOwner, Span);

/// The compile-time dependency limit: `min(call_depth, 32)`.
pub fn dependency_limit(call_depth: usize) -> u32 {
    call_depth.min(32) as u32
}

/// A result's depth evidence. `first` has `limit + 1` entries; index 0 is
/// unused (always `None`), index m in 1..=limit is the overflow site when the
/// result's own requests run at stack length m.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DepthEvidence<S = SiteSpan> {
    pub height: u32,
    pub first: Box<[Option<S>]>,
}

impl<S: Copy> DepthEvidence<S> {
    /// A result that requested nothing.
    pub fn leaf(limit: u32) -> Self {
        Self {
            height: 0,
            first: vec![None; limit as usize + 1].into_boxed_slice(),
        }
    }
    pub fn limit(&self) -> u32 {
        (self.first.len() - 1) as u32
    }
    /// Composes a result from its requests `(evidence, site)` in request order.
    pub fn compose<'a>(limit: u32, requests: impl IntoIterator<Item = (&'a Self, S)>) -> Self
    where
        S: 'a,
    {
        let mut evidence = Self::leaf(limit);
        let mut any = false;
        for (child, site) in requests {
            debug_assert_eq!(child.limit(), limit, "evidence under one limit");
            evidence.height = evidence.height.max(child.height + 1);
            for m in 1..=limit {
                let slot = &mut evidence.first[m as usize];
                if slot.is_none() {
                    *slot = child.request(m, site).err();
                }
            }
            any = true;
        }
        debug_assert!(any || evidence.first.iter().all(Option::is_none));
        evidence
    }
    /// Requesting this result at `site` from stack length `n`: `Err(site)` of
    /// the failure, or `Ok` when no request overflows.
    pub fn request(&self, n: u32, site: S) -> Result<(), S> {
        if n >= self.limit() {
            return Err(site);
        }
        match self.first[n as usize + 1] {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }
    /// Persisted form: height, limit, then per slot a presence byte and the site.
    pub fn encode(&self, out: &mut Vec<u8>, mut site: impl FnMut(&S, &mut Vec<u8>)) {
        out.extend(self.height.to_le_bytes());
        out.extend(self.limit().to_le_bytes());
        for slot in self.first.iter().skip(1) {
            match slot {
                Some(s) => {
                    out.push(1);
                    site(s, out);
                }
                None => out.push(0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A dependency graph: node → requests `(child, site)` in order.
    type Graph = Vec<Vec<(usize, u32)>>;

    /// A cache-free serial depth-first evaluation of `node` requested at
    /// `site` from stack length `n`: the overflowing site, if any.
    fn reference(graph: &Graph, limit: u32, node: usize, n: u32, site: u32) -> Result<(), u32> {
        if n >= limit {
            return Err(site);
        }
        for &(child, s) in &graph[node] {
            reference(graph, limit, child, n + 1, s)?;
        }
        Ok(())
    }

    /// Evidence computed bottom-up with sharing (every node once, as a cache
    /// would), whatever order nodes were first computed in.
    fn evidence(
        graph: &Graph,
        limit: u32,
        node: usize,
        memo: &mut HashMap<usize, DepthEvidence<u32>>,
    ) -> DepthEvidence<u32> {
        if let Some(e) = memo.get(&node) {
            return e.clone();
        }
        let children: Vec<_> = graph[node]
            .iter()
            .map(|&(c, s)| (evidence(graph, limit, c, memo), s))
            .collect();
        let e = DepthEvidence::compose(limit, children.iter().map(|(e, s)| (e, *s)));
        memo.insert(node, e.clone());
        e
    }

    /// The branching case: L=3, D requests leaf B and then C→E. D as a root
    /// and under one and two frames, with B or C evaluated first as a root.
    #[test]
    fn branching_depth_matches_a_cache_free_evaluation() {
        // 0=D, 1=B, 2=C, 3=E; sites are 10*(node)+ordinal.
        let graph: Graph = vec![vec![(1, 1), (2, 2)], vec![], vec![(3, 21)], vec![]];
        for warm in [vec![], vec![1], vec![2], vec![3, 2, 1]] {
            let mut memo = HashMap::new();
            for w in warm {
                evidence(&graph, 3, w, &mut memo);
            }
            let d = evidence(&graph, 3, 0, &mut memo);
            for n in 0..=4 {
                assert_eq!(
                    d.request(n, 99),
                    reference(&graph, 3, 0, n, 99),
                    "stack length {n}"
                );
            }
        }
        let d = evidence(&graph, 3, 0, &mut HashMap::new());
        assert_eq!(d.height, 2);
        assert_eq!(d.request(0, 99), Ok(()));
        assert_eq!(d.request(1, 99), Err(21), "D→C→E overflows at E's site");
        assert_eq!(d.request(2, 99), Err(1), "D's first request, B, overflows");
        assert_eq!(d.request(3, 99), Err(99), "zero budget: the request site");
    }

    /// Random DAGs with shared deep dependencies, every root at every stack
    /// length, against the cache-free evaluation.
    #[test]
    fn random_dags_agree_with_cache_free_evaluation() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let size = 2 + (next() % 14) as usize;
            let limit = 1 + (next() % 6) as u32;
            let mut graph: Graph = vec![vec![]; size];
            for (node, requests) in graph.iter_mut().enumerate() {
                for k in 0..(next() % 4) {
                    if node + 1 < size {
                        let child = node + 1 + (next() as usize % (size - node - 1));
                        requests.push((child, (node * 100) as u32 + k as u32));
                    }
                }
            }
            let mut memo = HashMap::new();
            for _ in 0..3 {
                evidence(&graph, limit, next() as usize % size, &mut memo);
            }
            for root in 0..size {
                let e = evidence(&graph, limit, root, &mut memo);
                for n in 0..=limit + 1 {
                    assert_eq!(e.request(n, 7777), reference(&graph, limit, root, n, 7777));
                }
            }
        }
    }

    #[test]
    fn near_limit_chain_fails_only_when_extended() {
        let chain = |len: usize| -> Graph {
            (0..len)
                .map(|i| {
                    if i + 1 < len {
                        vec![(i + 1, i as u32)]
                    } else {
                        vec![]
                    }
                })
                .collect()
        };
        let limit = dependency_limit(256);
        assert_eq!(limit, 32);
        let ok = evidence(&chain(32), limit, 0, &mut HashMap::new());
        assert_eq!(ok.height, 31);
        assert_eq!(ok.request(0, 999), Ok(()));
        let long = evidence(&chain(33), limit, 0, &mut HashMap::new());
        assert_eq!(long.request(0, 999), Err(31));
    }
}
