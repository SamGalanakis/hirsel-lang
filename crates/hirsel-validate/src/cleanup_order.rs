//! Root release order nests by scope (DROP-2): an exit releases each scope
//! it leaves innermost first, and within a scope its temporaries before its
//! names. Names release in descending order; temporaries of one scope reverse
//! the actual may-before facts. Observe only counted roots live when dropped.
use hirsel_intern::work::{self, Counter};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Order {
    /// The previous release: scope depth, temporary, root.
    last: Option<(usize, bool, usize)>,
    temporaries: BTreeMap<usize, (usize, usize, usize)>,
    ordinal: usize,
    bad: bool,
}
impl Order {
    pub fn observe(&mut self, root: usize, temporary: bool, depth: usize) {
        self.bad |= !releases_after(self.last, (depth, temporary, root));
        self.last = Some((depth, temporary, root));
        if temporary {
            self.temporaries
                .entry(root)
                .and_modify(|(_, last, _)| *last = self.ordinal)
                .or_insert((self.ordinal, self.ordinal, depth));
        }
        self.ordinal += 1;
    }
    pub fn invalid(&self, before: &BTreeSet<(usize, usize)>) -> bool {
        let mut bad = self.bad;
        for (&root, (first, _, depth)) in &self.temporaries {
            for &(_, next) in before.range((root, 0)..=(root, usize::MAX)) {
                work::count(Counter::ValidateCleanupRootRows);
                bad |= self
                    .temporaries
                    .get(&next)
                    .is_some_and(|(_, last, other)| other == depth && first < last);
            }
        }
        bad
    }
}

/// Whether `next` (scope depth, temporary, root) may be released right after
/// `previous`: never deeper, and within one depth temporaries before names,
/// names non-increasing. The relation is transitive, so adjacent pairs suffice.
pub(super) fn releases_after(
    previous: Option<(usize, bool, usize)>,
    next: (usize, bool, usize),
) -> bool {
    let Some((depth, temporary, root)) = previous else {
        return true;
    };
    match next.0.cmp(&depth) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => match (temporary, next.1) {
            (_, true) => temporary,
            (true, false) => true,
            (false, false) => next.2 <= root,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observed_order_matches_all_small_named_and_temporary_relations() {
        for length in 0..=4 {
            for sequence in 0..4usize.pow(length) {
                let ordered: Vec<_> = (0..length)
                    .map(|i| (sequence / 4usize.pow(i)) % 4)
                    .collect();
                for mask in 0..16 {
                    let before: BTreeSet<_> = (0..4)
                        .filter(|i| mask & (1 << i) != 0)
                        .map(|i| (i / 2, i % 2))
                        .collect();
                    let mut expected = false;
                    for (i, &a) in ordered.iter().enumerate() {
                        for &b in &ordered[i + 1..] {
                            expected |= if a < 2 && b < 2 {
                                before.contains(&(a, b))
                            } else if a >= 2 && b < 2 {
                                true
                            } else if a >= 2 && b >= 2 {
                                a < b
                            } else {
                                false
                            };
                        }
                    }
                    let mut actual = Order::default();
                    for &root in &ordered {
                        actual.observe(root, root < 2, 0);
                    }
                    assert_eq!(actual.invalid(&before), expected, "{ordered:?}/{before:?}");
                }
            }
        }
    }
    #[test]
    fn inner_scopes_release_before_outer_temporaries() {
        let order = |releases: &[(usize, bool, usize)]| {
            let mut order = Order::default();
            for &(root, temporary, depth) in releases {
                order.observe(root, temporary, depth);
            }
            order.invalid(&BTreeSet::new())
        };
        // An inner name before an outer temporary nests (DROP-2).
        assert!(!order(&[(7, false, 2), (3, true, 1), (1, false, 0)]));
        // The former global order, outer temporary first, does not.
        assert!(order(&[(3, true, 1), (7, false, 2), (1, false, 0)]));
        // Within one scope temporaries still precede names.
        assert!(order(&[(5, false, 1), (3, true, 1)]));
        // May-before facts bind temporaries of one scope only.
        let mut nested = Order::default();
        nested.observe(8, true, 2);
        nested.observe(9, true, 1);
        assert!(!nested.invalid(&[(8, 9)].into()));
        let mut flat = Order::default();
        flat.observe(8, true, 1);
        flat.observe(9, true, 1);
        assert!(flat.invalid(&[(8, 9)].into()));
    }
}
