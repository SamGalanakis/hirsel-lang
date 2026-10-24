//! Exact deadline index, following Glommio reactor.rs:80–140's ID/time pair.
//! Unlike a lazy heap, removal releases both entries immediately. The token is
//! the stable registration tie, never an address. Clock domains use separate
//! indexes; equality is due (Hirsel's inclusive deadline contract).
use crate::id_map::IdMap;
use std::collections::BTreeSet;

pub(crate) struct Deadlines<D> {
    by_token: IdMap<D>,
    ordered: BTreeSet<(D, u64)>,
}

impl<D> Default for Deadlines<D> {
    fn default() -> Self {
        Self {
            by_token: Default::default(),
            ordered: Default::default(),
        }
    }
}

impl<D: Copy + Ord> Deadlines<D> {
    pub(crate) fn insert(&mut self, token: u64, at: D) {
        self.remove(token);
        self.by_token.insert(token, at);
        self.ordered.insert((at, token));
    }

    pub(crate) fn remove(&mut self, token: u64) -> Option<D> {
        let at = self.by_token.remove(&token)?;
        assert!(self.ordered.remove(&(at, token)));
        Some(at)
    }

    pub(crate) fn next(&self) -> Option<D> {
        self.ordered.first().map(|&(at, _)| at)
    }

    pub(crate) fn pop_due(&mut self, now: D) -> Option<(D, u64)> {
        let &(at, token) = self.ordered.first()?;
        if at > now {
            return None;
        }
        self.remove(token);
        Some((at, token))
    }

    pub(crate) fn len(&self) -> usize {
        self.by_token.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.by_token.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn stored(&self) -> (usize, usize) {
        (self.by_token.len(), self.ordered.len())
    }
}

#[cfg(test)]
#[path = "deadlines/modules.rs"]
mod tests;
