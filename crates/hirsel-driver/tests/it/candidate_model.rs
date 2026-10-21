//! Independent small state model for future public-operation history checks.
//! TS-Go session.go:1393-1425 lends expected-parent adoption; generation,
//! checked verdict and operation retry rules follow Hirsel's txn design.
use std::collections::BTreeMap;
use std::sync::Arc;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub workspace: u64,
    pub generation: u64,
    pub contents: Arc<BTreeMap<String, String>>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Pending,
    CompletePass,
    CompleteReject,
    Cancelled,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Candidate {
    parent: Token,
    contents: Arc<BTreeMap<String, String>>,
    state: State,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Promotion {
    Promoted(Token),
    Conflict(Token),
    Ineligible,
    IdConflict,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct PromotionRecord {
    candidate: u64,
    expected: Token,
    result: Promotion,
}
pub struct Model {
    head: Token,
    candidates: BTreeMap<u64, Candidate>,
    promotions: BTreeMap<u64, PromotionRecord>,
}
impl Model {
    pub fn new(workspace: u64, contents: BTreeMap<String, String>) -> Self {
        Self {
            head: Token {
                workspace,
                generation: 1,
                contents: Arc::new(contents),
            },
            candidates: BTreeMap::new(),
            promotions: BTreeMap::new(),
        }
    }
    pub fn head(&self) -> Token {
        self.head.clone()
    }
    pub fn capture(&mut self, contents: BTreeMap<String, String>) -> Token {
        self.head = Token {
            workspace: self.head.workspace,
            generation: self.head.generation.checked_add(1).unwrap(),
            contents: Arc::new(contents),
        };
        self.head()
    }
    pub fn submit(&mut self, id: u64, parent: Token, contents: BTreeMap<String, String>) -> bool {
        let new = Candidate {
            parent,
            contents: Arc::new(contents),
            state: State::Pending,
        };
        if let Some(old) = self.candidates.get(&id) {
            return old.parent == new.parent && old.contents == new.contents;
        }
        self.candidates.insert(id, new);
        true
    }
    pub fn publish(&mut self, id: u64, pass: bool) {
        if let Some(c) = self.candidates.get_mut(&id) {
            if c.state == State::Pending {
                c.state = if pass {
                    State::CompletePass
                } else {
                    State::CompleteReject
                };
            }
        }
    }
    pub fn cancel(&mut self, id: u64) {
        if let Some(c) = self.candidates.get_mut(&id) {
            if c.state == State::Pending {
                c.state = State::Cancelled;
            }
        }
    }
    pub fn result(&self, id: u64) -> Option<State> {
        self.candidates.get(&id).map(|c| c.state)
    }
    pub fn promote(&mut self, operation: u64, id: u64, expected: Token) -> Promotion {
        if let Some(record) = self.promotions.get(&operation) {
            return if record.candidate == id && record.expected == expected {
                record.result.clone()
            } else {
                Promotion::IdConflict
            };
        }
        let result = match self.candidates.get(&id) {
            Some(c) if self.head != expected || c.parent != expected => {
                Promotion::Conflict(self.head())
            }
            Some(c) if c.state == State::CompletePass => {
                self.head = Token {
                    workspace: self.head.workspace,
                    generation: self.head.generation.checked_add(1).unwrap(),
                    contents: c.contents.clone(),
                };
                Promotion::Promoted(self.head())
            }
            _ => Promotion::Ineligible,
        };
        self.promotions.insert(
            operation,
            PromotionRecord {
                candidate: id,
                expected,
                result: result.clone(),
            },
        );
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn files(n: u64) -> BTreeMap<String, String> {
        BTreeMap::from([("main.hir".into(), format!("pub fn main() -> u64 {{ {n} }}"))])
    }
    #[test]
    fn exact_parent_retry_aba_and_independent_candidate_states() {
        let mut m = Model::new(1, files(1));
        let a = m.head();
        assert!(m.submit(1, a.clone(), files(2)));
        assert!(m.submit(2, a.clone(), files(3)));
        m.publish(1, true);
        m.publish(2, false);
        let p = m.promote(10, 1, a.clone());
        assert!(matches!(p, Promotion::Promoted(_)));
        assert_eq!(m.promote(10, 1, a.clone()), p);
        assert_eq!(m.head().generation, 2);
        assert_eq!(m.promote(10, 2, a.clone()), Promotion::IdConflict);
        let again = m.capture(files(1));
        assert_eq!(a.contents, again.contents);
        assert_ne!(a.generation, again.generation);
        assert_eq!(m.promote(11, 1, a), Promotion::Conflict(again));
        assert_eq!(m.result(2), Some(State::CompleteReject));
    }
    #[test]
    fn terminal_waiter_state_wins_publication_cancel_race() {
        let mut m = Model::new(1, files(1));
        let a = m.head();
        m.submit(1, a.clone(), files(2));
        m.submit(2, a.clone(), files(2));
        m.cancel(1);
        m.publish(1, true);
        m.publish(2, true);
        m.cancel(2);
        assert_eq!(m.result(1), Some(State::Cancelled));
        assert_eq!(m.result(2), Some(State::CompletePass));
        assert_eq!(m.promote(1, 1, a.clone()), Promotion::Ineligible);
        assert!(matches!(m.promote(2, 2, a), Promotion::Promoted(_)));
    }
    #[test]
    fn deterministic_barrier_promotions_have_one_winner() {
        use std::sync::{Barrier, Mutex};
        let mut m = Model::new(1, files(1));
        let a = m.head();
        for id in [1, 2] {
            m.submit(id, a.clone(), files(2));
            m.publish(id, true);
        }
        let model = Arc::new(Mutex::new(m));
        let barrier = Arc::new(Barrier::new(3));
        let workers: Vec<_> = [1, 2]
            .into_iter()
            .map(|id| {
                let (model, barrier, a) = (model.clone(), barrier.clone(), a.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    model.lock().unwrap().promote(id, id, a)
                })
            })
            .collect();
        barrier.wait();
        let outcomes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(
            outcomes
                .iter()
                .filter(|p| matches!(p, Promotion::Promoted(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|p| matches!(p, Promotion::Conflict(_)))
                .count(),
            1
        );
        assert_eq!(model.lock().unwrap().head().generation, 2);
    }
}

/// Union-and-lookup oracle, deliberately independent of the ordered merge.
pub fn read_difference<K: Clone + Ord, V: Copy + Eq>(
    before: &BTreeMap<K, V>,
    after: &BTreeMap<K, V>,
) -> Vec<(K, Option<V>, Option<V>)> {
    let keys: std::collections::BTreeSet<_> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter_map(|key| {
            let old = before.get(key).copied();
            let new = after.get(key).copied();
            (old != new).then(|| (key.clone(), old, new))
        })
        .collect()
}
