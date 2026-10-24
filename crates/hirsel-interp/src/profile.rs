//! Optional, exclusive step counts by definition, outside semantic observations.
use hirsel_syntax::DefPath;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

type Counts = Rc<RefCell<BTreeMap<DefPath, u64>>>;
type Request = (DefPath, Counts);
thread_local! {
    static ACTIVE: RefCell<Option<Request>> = const { RefCell::new(None) };
}

/// Count steps on the current thread for executions rooted at `entry`, including their tasks.
/// Other entry points (such as lazy compile-time evaluation) are excluded.
/// Counts are exclusive: a callee's steps belong to the callee. Profiling neither
/// changes the step bound nor records events in the semantic trace. Nested scopes
/// collect independently; unwinding restores the enclosing scope.
pub fn with_step_profile<T>(
    entry: &DefPath,
    run: impl FnOnce() -> T,
) -> (T, BTreeMap<DefPath, u64>) {
    struct Restore(Option<Request>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with(|active| *active.borrow_mut() = self.0.take());
        }
    }
    let counts = Counts::default();
    let restore =
        Restore(ACTIVE.with(|active| active.replace(Some((entry.clone(), counts.clone())))));
    let result = run();
    drop(restore);
    let counts = Rc::try_unwrap(counts)
        .expect("profile retained after execution")
        .into_inner();
    (result, counts)
}

pub(crate) struct Profile(Option<Counts>);
impl Profile {
    pub(crate) fn active(entry: &DefPath) -> Self {
        Self(ACTIVE.with(|active| {
            active
                .borrow()
                .as_ref()
                .and_then(|(wanted, counts)| (wanted == entry).then(|| counts.clone()))
        }))
    }
    pub(crate) fn add(&self, definition: &DefPath, steps: u64) {
        if steps != 0
            && let Some(counts) = &self.0
        {
            *counts.borrow_mut().entry(definition.clone()).or_default() += steps;
        }
    }
}
