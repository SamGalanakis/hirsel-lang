//! The S1 worklist (design §1c): work items with canonical keys, taken in
//! key order round by round, with idempotent replay of cached registrations
//! and retirement of registrations a revision no longer reaches.
use hirsel_syntax::DefId;
use std::collections::{BTreeMap, BTreeSet};

/// What an S1 work item does.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WorkKind {
    Generate,
    EvaluateConstant,
    PrepareComptime,
    SelectInstance,
}

/// `(kind, declaration DefPath, GeneratedActualKey, selecting site)`. DefId
/// orders by DefPath and `A` (the interned actual key) by its spelling, so
/// key order never depends on discovery or worker order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkKey<A> {
    pub kind: WorkKind,
    pub declaration: DefId,
    pub actuals: Option<A>,
    /// The selecting site: the selecting definition and an item-relative offset.
    pub site: Option<(DefId, u32)>,
}

/// One revision's worklist over cached registrations `R`.
pub struct Worklist<A, R> {
    registrations: BTreeMap<WorkKey<A>, R>,
    visited: BTreeSet<WorkKey<A>>,
    frontier: BTreeSet<WorkKey<A>>,
    rounds: u32,
}

impl<A: Ord + Copy, R> Default for Worklist<A, R> {
    fn default() -> Self {
        Self {
            registrations: BTreeMap::new(),
            visited: BTreeSet::new(),
            frontier: BTreeSet::new(),
            rounds: 0,
        }
    }
}

/// What one round did with an item.
pub enum Step<A> {
    /// Replay the cached registration (it must be idempotent).
    Replay(WorkKey<A>),
    /// Compute it; record the registration with `Worklist::record`.
    Compute(WorkKey<A>),
}

impl<A: Ord + Copy, R> Worklist<A, R> {
    /// Starts a revision: nothing visited, an empty frontier; cached
    /// registrations stay for replay.
    pub fn begin(&mut self) {
        self.visited.clear();
        self.frontier.clear();
        self.rounds = 0;
    }
    /// Admits an item to the next round unless this revision already took it.
    pub fn admit(&mut self, key: WorkKey<A>) -> bool {
        !self.visited.contains(&key) && self.frontier.insert(key)
    }
    pub fn is_settled(&self) -> bool {
        self.frontier.is_empty()
    }
    pub fn rounds(&self) -> u32 {
        self.rounds
    }
    /// Takes the frontier in key order; each item is replayed when a
    /// registration is cached and `valid` accepts it, else computed.
    pub fn round(&mut self, mut valid: impl FnMut(&WorkKey<A>, &R) -> bool) -> Vec<Step<A>> {
        self.rounds += 1;
        let frontier = std::mem::take(&mut self.frontier);
        frontier
            .into_iter()
            .map(|key| {
                self.visited.insert(key);
                match self.registrations.get(&key) {
                    Some(r) if valid(&key, r) => Step::Replay(key),
                    _ => Step::Compute(key),
                }
            })
            .collect()
    }
    pub fn registration(&self, key: &WorkKey<A>) -> Option<&R> {
        self.registrations.get(key)
    }
    pub fn record(&mut self, key: WorkKey<A>, registration: R) {
        self.registrations.insert(key, registration);
    }
    /// Removes the registrations this revision did not visit, in key order;
    /// their subscribers must be invalidated.
    pub fn retire(&mut self) -> Vec<(WorkKey<A>, R)> {
        let stale: Vec<_> = self
            .registrations
            .keys()
            .filter(|k| !self.visited.contains(*k))
            .copied()
            .collect();
        stale
            .into_iter()
            .map(|k| {
                let r = self.registrations.remove(&k).unwrap();
                (k, r)
            })
            .collect()
    }
    pub fn len(&self) -> usize {
        self.registrations.len()
    }
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::{DefPath, ItemPath, PackageId};
    fn def(name: &str) -> DefId {
        DefId::intern(&DefPath(ItemPath {
            package: PackageId {
                name: "w".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        }))
    }
    fn select(name: &str, actual: u32, site: u32) -> WorkKey<u32> {
        WorkKey {
            kind: WorkKind::SelectInstance,
            declaration: def(name),
            actuals: Some(actual),
            site: Some((def("main"), site)),
        }
    }

    /// Nested generators: `g` selects `h` at two actuals, `h` selects `k`.
    /// Rounds run in key order whatever order items were admitted in; a
    /// second revision replays every registration; a third revision that no
    /// longer reaches `h` at 2 retires it and what only it selected.
    #[test]
    fn rounds_take_key_order_replay_and_retire() {
        let graph = |key: &WorkKey<u32>, reach_h2: bool| -> Vec<WorkKey<u32>> {
            match (key.declaration.path().0.name.as_str(), key.actuals) {
                ("g", _) if reach_h2 => vec![select("h", 2, 2), select("h", 1, 1)],
                ("g", _) => vec![select("h", 1, 1)],
                ("h", Some(a)) => vec![select("k", a * 10, 3)],
                _ => vec![],
            }
        };
        let mut list: Worklist<u32, Vec<WorkKey<u32>>> = Worklist::default();
        let mut run = |list: &mut Worklist<u32, Vec<WorkKey<u32>>>, reach_h2: bool| {
            list.begin();
            list.admit(select("g", 0, 0));
            let mut order = vec![];
            let mut computed = 0;
            while !list.is_settled() {
                for step in list.round(|_, _| true) {
                    let (key, admits) = match step {
                        Step::Replay(key) => (key, list.registration(&key).unwrap().clone()),
                        Step::Compute(key) => {
                            computed += 1;
                            let admits = graph(&key, reach_h2);
                            list.record(key, admits.clone());
                            (key, admits)
                        }
                    };
                    order.push(key);
                    for a in admits {
                        list.admit(a);
                    }
                }
            }
            (order, computed, list.retire())
        };
        let (order, computed, retired) = run(&mut list, true);
        assert_eq!(computed, 5);
        assert!(retired.is_empty());
        assert_eq!(
            order,
            vec![
                select("g", 0, 0),
                select("h", 1, 1),
                select("h", 2, 2),
                select("k", 10, 3),
                select("k", 20, 3)
            ]
        );
        let (again, computed, _) = run(&mut list, true);
        assert_eq!((again, computed), (order, 0), "idempotent replay");
        // g's own registration is replayed, so recompute it as its input changed.
        list.record(select("g", 0, 0), vec![select("h", 1, 1)]);
        let (_, _, retired) = run(&mut list, false);
        let retired: Vec<_> = retired.into_iter().map(|(k, _)| k).collect();
        assert_eq!(retired, vec![select("h", 2, 2), select("k", 20, 3)]);
        assert_eq!(list.len(), 3);
    }
}
