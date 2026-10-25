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
