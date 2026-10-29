//! Borrowed environments name the existing owner. They never create, move or
//! destroy a resource token merely because the evaluator copied a borrow view.
use super::{Owner, Resources, Storage};
impl<const OWNED: bool> Resources<OWNED> {
    pub(super) fn has_borrows(&self, owner: &Owner) -> bool {
        self.borrows.keys().any(|at| {
            at.task == owner.task && at.storage == owner.storage && at.path.starts_with(&owner.path)
        })
    }
    pub(super) fn borrow_target(&self, owner: &Owner) -> Option<Owner> {
        let (at, target) = self
            .borrows
            .iter()
            .filter(|(at, _)| {
                at.task == owner.task
                    && at.storage == owner.storage
                    && owner.path.starts_with(&at.path)
            })
            .max_by_key(|(at, _)| at.path.len())?;
        let mut target = target.clone();
        target.path.extend_from_slice(&owner.path[at.path.len()..]);
        Some(target)
    }
    pub(super) fn forget_borrows(&mut self, owner: &Owner) {
        self.borrows.retain(|at, _| {
            !(at.task == owner.task
                && at.storage == owner.storage
                && at.path.starts_with(&owner.path))
        });
    }
    pub(super) fn move_borrows(&mut self, from: &Owner, to: &Owner, copy: bool) {
        let views: Vec<_> = self
            .borrows
            .iter()
            .filter(|(at, _)| {
                at.task == from.task
                    && at.storage == from.storage
                    && at.path.starts_with(&from.path)
            })
            .map(|(at, target)| {
                let mut destination = to.clone();
                destination
                    .path
                    .extend_from_slice(&at.path[from.path.len()..]);
                (destination, target.clone())
            })
            .collect();
        if !copy {
            self.forget_borrows(from);
        }
        self.forget_borrows(to);
        for (at, target) in views {
            self.insert_borrow(at, target);
        }
    }
    pub(super) fn insert_borrow(&mut self, at: Owner, target: Owner) {
        if self.borrows.len() >= self.task_limit {
            self.unsupported("borrowed environment metadata budget exhausted");
            return;
        }
        self.borrows.insert(at, target);
    }
    pub(super) fn retired_frame(&mut self, frame: u64) {
        self.borrows
            .retain(|at, _| !matches!(at.storage, Storage::Slot {frame: id,..} if id == frame));
    }
}
