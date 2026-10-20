//! A run's parked tasks in poll order: waits without a deadline first, then timers by
//! deadline, ties in suspension order. That is the order a stable sort by deadline
//! keeps at every poll, since a wait's deadline is fixed while it parks. A watched wait
//! is polled only when its layer reports it woken, so a poll costs the woken and
//! unwatched waits, not every parked task.
use crate::Woken;
use crate::frame::Frame;
use crate::observation::FailureRecord;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A task's id, stack and failure chain.
pub(crate) type Task = (u64, Vec<Frame>, Vec<FailureRecord>);
/// A parked task's place in poll order: its wait's deadline and its suspension ordinal.
pub(crate) type Key = (Option<u64>, u64);

#[derive(Default)]
pub(crate) struct Parked {
    suspended: u64,
    tasks: BTreeMap<Key, (Task, u64)>,
    unwatched: BTreeSet<Key>,
    watched: HashMap<u64, Key>,
}

impl Parked {
    /// Park `task` on the wait `token`.
    pub fn push(&mut self, task: Task, token: u64, deadline: Option<u64>, watched: bool) {
        let key = (deadline, self.suspended);
        self.suspended += 1;
        if watched {
            self.watched.insert(token, key);
        } else {
            self.unwatched.insert(key);
        }
        self.tasks.insert(key, (task, token));
    }

    /// The tasks a poll asks about, in poll order.
    pub fn due(&self, woken: &Woken) -> Vec<Key> {
        match woken {
            _ if self.tasks.is_empty() => vec![],
            Woken::Every => self.tasks.keys().copied().collect(),
            Woken::Tokens(tokens) => {
                let mut keys: Vec<Key> = self.unwatched.iter().copied().collect();
                keys.extend(tokens.iter().filter_map(|token| self.watched.get(token)));
                keys.sort_unstable();
                keys.dedup();
                keys
            }
        }
    }

    pub fn get_mut(&mut self, key: Key) -> Option<&mut Task> {
        self.tasks.get_mut(&key).map(|(task, _)| task)
    }

    pub fn remove(&mut self, key: Key) -> Option<Task> {
        let (task, token) = self.tasks.remove(&key)?;
        if !self.unwatched.remove(&key) {
            self.watched.remove(&token);
        }
        Some(task)
    }

    /// Every parked task, in poll order.
    pub fn iter(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values().map(|(task, _)| task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64) -> Task {
        (id, vec![], vec![])
    }

    fn ids(parked: &mut Parked, woken: &Woken) -> Vec<u64> {
        let keys = parked.due(woken);
        keys.into_iter()
            .map(|key| parked.get_mut(key).unwrap().0)
            .collect()
    }

    #[test]
    fn poll_order_is_a_stable_deadline_sort_of_suspension_order() {
        let mut parked = Parked::default();
        parked.push(task(1), 10, Some(30), false);
        parked.push(task(2), 11, None, false);
        parked.push(task(3), 12, Some(10), false);
        parked.push(task(4), 13, Some(10), false);
        parked.push(task(5), 14, None, false);
        assert_eq!(ids(&mut parked, &Woken::Every), [2, 5, 3, 4, 1]);
        // A task parked again goes behind its deadline's ties.
        let key = parked.due(&Woken::Every)[2];
        let three = parked.remove(key).unwrap();
        parked.push(three, 15, Some(10), false);
        assert_eq!(ids(&mut parked, &Woken::Every), [2, 5, 4, 3, 1]);
    }

    #[test]
    fn a_watched_wait_is_asked_about_only_when_woken() {
        let mut parked = Parked::default();
        parked.push(task(1), 10, None, true);
        parked.push(task(2), 11, None, false);
        parked.push(task(3), 12, Some(5), true);
        parked.push(task(4), 13, None, true);
        assert_eq!(ids(&mut parked, &Woken::Tokens(vec![])), [2]);
        // Woken tokens are polled in poll order, not token order; unknown tokens are ignored.
        assert_eq!(
            ids(&mut parked, &Woken::Tokens(vec![12, 99, 10, 12])),
            [1, 2, 3]
        );
        assert_eq!(ids(&mut parked, &Woken::Every), [1, 2, 4, 3]);
        let key = parked.due(&Woken::Tokens(vec![10]))[0];
        parked.remove(key).unwrap();
        assert_eq!(ids(&mut parked, &Woken::Tokens(vec![10, 13])), [2, 4]);
        assert_eq!(parked.iter().map(|t| t.0).collect::<Vec<_>>(), [2, 4, 3]);
    }
}
