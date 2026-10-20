//! A task's pending `Shared` releases (design §5 Deferred shared releases).
//!
//! A release that does not find the root's count at 1 is deferred to the releasing task's
//! list, coalescing per root; a later dup of that root by the same task cancels one pending
//! release instead of incrementing. The list is applied in the entered phase of the task's
//! next op and drained by its end's cleanup phases, so a segment has no effect another task
//! can observe.
//!
//! **Walks.** A root that a segment frees (its release found the count at 1) is walked in the
//! segment, and the walk's nested releases follow the same rule. A root that an entered or
//! cleanup phase brings to 0 is walked as the tail of that phase, and that walk applies its
//! nested releases at once (a nested root freed in turn is walked the same way): which task's
//! entry applies a root's last release depends on the order of different tasks' phases, which
//! the tape does not record, so a nested release deferred to the walker's next phase could
//! land, in a replay, on a task with no further admitted phase and strand a recorded thaw.
//! Attached to the phase that freed its root, it replays whenever that phase is admitted. The
//! reference executor's `deferring_a_phase_walk_strands_nested_releases` shows the deferring
//! rule failing.

/// Pending releases by root, in first-deferred order.
#[derive(Clone, Debug)]
pub struct Pending<R> {
    roots: Vec<(R, u32)>,
}

impl<R> Default for Pending<R> {
    fn default() -> Self {
        Pending { roots: Vec::new() }
    }
}

impl<R: Eq> Pending<R> {
    pub fn defer(&mut self, root: R) {
        match self.roots.iter_mut().find(|(pending, _)| *pending == root) {
            Some((_, count)) => *count += 1,
            None => self.roots.push((root, 1)),
        }
    }

    /// A dup of `root` by this task: whether it cancelled a pending release (the count is
    /// then not incremented).
    pub fn dup(&mut self, root: &R) -> bool {
        let Some(at) = self.roots.iter().position(|(pending, _)| pending == root) else {
            return false;
        };
        self.roots[at].1 -= 1;
        if self.roots[at].1 == 0 {
            self.roots.remove(at);
        }
        true
    }

    /// Everything pending, to apply now, each root with its release count.
    pub fn take(&mut self) -> Vec<(R, u32)> {
        std::mem::take(&mut self.roots)
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}
