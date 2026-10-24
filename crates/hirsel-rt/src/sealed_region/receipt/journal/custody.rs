//! One source-credit enrollment order, independent of allocation map order.
//!
//! The live index owns credits. Batches expose nonowning words/identities for
//! source mutation ordinals. Keeping a credit preserves its enrollment; taking
//! it removes custody without retirement. This is not a type or shape graph.
use super::{AllocationId, Error, Header};
use crate::region::sealed_cleanup::Credits;
use crate::sealed_region::transaction::JournalCredits;
use std::collections::BTreeMap;

type CreditId = u64;

#[derive(Default)]
struct Batch {
    words: Vec<*mut Header>,
    ids: Vec<CreditId>,
}

pub(super) struct Custody {
    batches: BTreeMap<AllocationId, Batch>,
    live: BTreeMap<CreditId, *mut Header>,
    next: Option<CreditId>,
}

/// Staged words own nothing until the paired semantic event commits.
pub(super) struct Change {
    batch: Batch,
    remove: Vec<CreditId>,
    retired: Vec<*mut Header>,
    transferred: Vec<*mut Header>,
    next: Option<CreditId>,
}

impl Custody {
    pub(super) fn new() -> Self {
        Self {
            batches: BTreeMap::new(),
            live: BTreeMap::new(),
            next: Some(0),
        }
    }

    pub(super) fn holders(&self, id: AllocationId) -> Option<&[*mut Header]> {
        self.batches.get(&id).map(|batch| batch.words.as_slice())
    }

    fn fresh(&self, count: usize) -> Result<(Vec<CreditId>, Option<CreditId>), Error> {
        if count == 0 {
            return Ok((Vec::new(), self.next));
        }
        let count = u64::try_from(count).map_err(|_| Error::CreditIdExhausted)?;
        let first = self.next.ok_or(Error::CreditIdExhausted)?;
        let last = first
            .checked_add(count - 1)
            .ok_or(Error::CreditIdExhausted)?;
        Ok(((first..=last).collect(), last.checked_add(1)))
    }

    pub(super) fn allocation(&self, additions: &[*mut Header]) -> Result<Change, Error> {
        if additions.iter().any(|holder| holder.is_null()) {
            return Err(Error::InvalidHolder);
        }
        let (ids, next) = self.fresh(additions.len())?;
        Ok(Change {
            batch: Batch {
                words: additions.to_vec(),
                ids,
            },
            remove: Vec::new(),
            retired: Vec::new(),
            transferred: Vec::new(),
            next,
        })
    }

    pub(super) fn replacement(
        &self,
        id: AllocationId,
        keep: &[usize],
        take: &[usize],
        additions: &[*mut Header],
    ) -> Result<Change, Error> {
        let old = self
            .batches
            .get(&id)
            .ok_or(Error::Semantic(super::semantic::Error::UnknownAllocation))?;
        let invalid = |indices: &[usize]| {
            indices.windows(2).any(|pair| pair[0] >= pair[1])
                || indices
                    .last()
                    .is_some_and(|&index| index >= old.words.len())
        };
        if invalid(keep)
            || invalid(take)
            || keep.iter().any(|index| take.binary_search(index).is_ok())
        {
            return Err(Error::InvalidRetention);
        }
        let mut change = self.allocation(additions)?;
        let mut words = Vec::with_capacity(keep.len());
        words.extend(keep.iter().map(|&index| old.words[index]));
        words.append(&mut change.batch.words);
        let mut ids = Vec::with_capacity(keep.len());
        ids.extend(keep.iter().map(|&index| old.ids[index]));
        ids.append(&mut change.batch.ids);
        change.batch = Batch { words, ids };
        change.transferred = take.iter().map(|&index| old.words[index]).collect();
        let mut retired = Vec::new();
        let mut kept = keep.iter().copied().peekable();
        let mut taken = take.iter().copied().peekable();
        for (index, &credit) in old.ids.iter().enumerate() {
            if kept.peek() == Some(&index) {
                kept.next();
                continue;
            }
            change.remove.push(credit);
            if taken.peek() == Some(&index) {
                taken.next();
            } else {
                retired.push((credit, old.words[index]));
            }
        }
        retired.sort_unstable_by_key(|&(credit, _)| std::cmp::Reverse(credit));
        change.retired = retired.into_iter().map(|(_, word)| word).collect();
        Ok(change)
    }

    /// Called only after metadata commits, with no intervening journal event.
    /// Returns taken words and omitted credits in their retirement order.
    pub(super) fn commit(
        &mut self,
        id: AllocationId,
        change: Change,
    ) -> (Vec<*mut Header>, Vec<*mut Header>) {
        for credit in change.remove {
            self.live.remove(&credit);
        }
        for (&credit, &word) in change.batch.ids.iter().zip(&change.batch.words) {
            self.live.insert(credit, word);
        }
        self.batches.insert(id, change.batch);
        self.next = change.next;
        (change.transferred, change.retired)
    }

    /// Consume live credits newest first without allocating a flattened list.
    pub(super) unsafe fn retire(&mut self, ctx: &mut crate::TaskCtx) {
        while let Some((_, holder)) = self.live.pop_last() {
            // SAFETY: the live index uniquely owns this resource-free credit.
            unsafe { Credits::release_one(ctx, holder) };
        }
    }
}

impl JournalCredits for Custody {
    fn snapshot(&self, credits: &mut Credits) {
        for holder in self.live.values().rev() {
            credits.journal_snapshot(std::slice::from_ref(holder));
        }
    }

    fn disarm(&mut self) {
        self.live.clear();
        self.batches.clear();
    }
}

#[cfg(all(test, feature = "diagnostic"))]
pub(super) fn exhaust_for_test(custody: &mut Custody, next: Option<u64>) {
    custody.next = next;
}
