//! Immutable lexical-scope interval index, queried in original local order.
use super::*;
pub(super) struct ScopeLocals {
    ranges: Vec<(usize, usize)>,
    locals: Vec<(usize, usize)>,
}
impl ScopeLocals {
    pub fn new(
        parents: impl IntoIterator<Item = Option<ScopeId>>,
        locals: impl IntoIterator<Item = ScopeId>,
    ) -> Self {
        let parents: Vec<_> = parents.into_iter().collect();
        let mut children = vec![vec![]; parents.len()];
        let mut roots = vec![];
        for (i, parent) in parents.into_iter().enumerate() {
            work::count(Counter::ValidateScopeLocalRows);
            if let Some(parent) = parent {
                children[parent.0 as usize].push(i)
            } else {
                roots.push(i)
            }
        }
        let mut ranges = vec![(0, 0); children.len()];
        let mut work: Vec<_> = roots.into_iter().rev().map(|i| (i, false)).collect();
        let mut clock = 0;
        while let Some((i, leave)) = work.pop() {
            work::count(Counter::ValidateScopeLocalRows);
            if leave {
                ranges[i].1 = clock;
            } else {
                ranges[i].0 = clock;
                clock += 1;
                work.push((i, true));
                work.extend(children[i].iter().rev().map(|i| (*i, false)));
            }
        }
        let mut locals: Vec<_> = locals
            .into_iter()
            .enumerate()
            .map(|(i, scope)| {
                work::count(Counter::ValidateScopeLocalRows);
                (ranges[scope.0 as usize].0, i)
            })
            .collect();
        locals.sort_unstable_by(|a, b| {
            work::count(Counter::ValidateScopeLocalRows);
            a.cmp(b)
        });
        Self { ranges, locals }
    }
    pub fn within(&self, scope: ScopeId) -> Vec<usize> {
        let (lo, hi) = self.ranges[scope.0 as usize];
        let start = self.locals.partition_point(|(rank, _)| {
            work::count(Counter::ValidateScopeLocalRows);
            *rank < lo
        });
        let end = self.locals.partition_point(|(rank, _)| {
            work::count(Counter::ValidateScopeLocalRows);
            *rank < hi
        });
        let mut selected: Vec<_> = self.locals[start..end]
            .iter()
            .map(|(_, local)| {
                work::count(Counter::ValidateScopeLocalRows);
                *local
            })
            .collect();
        selected.sort_unstable_by(|a, b| {
            work::count(Counter::ValidateScopeLocalRows);
            a.cmp(b)
        });
        selected
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_intervals_preserve_local_order_across_siblings_and_roots() {
        let index = ScopeLocals::new(
            [
                None,
                Some(ScopeId(0)),
                Some(ScopeId(0)),
                None,
                Some(ScopeId(3)),
            ],
            [3, 1, 0, 4, 2, 1].map(ScopeId),
        );
        assert_eq!(index.within(ScopeId(0)), vec![1, 2, 4, 5]);
        assert_eq!(index.within(ScopeId(1)), vec![1, 5]);
        assert_eq!(index.within(ScopeId(2)), vec![4]);
        assert_eq!(index.within(ScopeId(3)), vec![0, 3]);
        assert_eq!(index.within(ScopeId(4)), vec![3]);
    }
}
