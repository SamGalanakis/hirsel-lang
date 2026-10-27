//! Interned lookup with source-ordered iteration. Rust's existing DefMap keeps
//! hot reads independent of path length; the immutable row slice keeps output
//! ordering independent of the hash table and session's assigned identities.
use super::*;

pub(super) struct Inputs {
    rows: Box<[(DefId, Input)]>,
    positions: hirsel_syntax::DefMap<usize>,
}
impl Inputs {
    pub(super) fn new(mut rows: Vec<(DefId, Input)>) -> Self {
        rows.sort_unstable_by_key(|(id, _)| *id);
        rows.dedup_by_key(|(id, _)| *id);
        let positions = rows
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (*id, i))
            .collect();
        Self {
            rows: rows.into_boxed_slice(),
            positions,
        }
    }
    pub(super) fn retained_bytes(&self) -> Option<u64> {
        // The same hashbrown bucket bound as source_index::hash. Controls
        // and the trailing SIMD group are charged alongside every bucket.
        let layout = std::alloc::Layout::new::<(DefId, usize)>();
        let buckets = if self.positions.capacity() == 0 {
            0
        } else {
            self.positions
                .capacity()
                .checked_add(1)?
                .checked_next_power_of_two()?
        };
        let hash = buckets
            .checked_mul(layout.size().checked_add(1)?)?
            .checked_add(layout.align())?
            .checked_add(32)?;
        u64::try_from(hash.checked_add(size_of_val(&*self.rows))?).ok()
    }
    pub(super) fn get(&self, path: &DefPath) -> Option<&Input> {
        self.get_id(DefId::get(path)?)
    }
    pub(super) fn get_id(&self, id: DefId) -> Option<&Input> {
        Some(&self.rows[*self.positions.get(&id)?].1)
    }
    pub(super) fn contains_key(&self, path: &DefPath) -> bool {
        self.get(path).is_some()
    }
    pub(super) fn iter(&self) -> Iter<'_> {
        Iter(self.rows.iter())
    }
    pub(super) fn keys(&self) -> impl Iterator<Item = &DefPath> {
        self.rows.iter().map(|(id, _)| id.path())
    }
}
impl std::ops::Index<&DefPath> for Inputs {
    type Output = Input;
    fn index(&self, path: &DefPath) -> &Input {
        self.get(path).expect("captured Lower input")
    }
}
pub(super) struct Iter<'a>(std::slice::Iter<'a, (DefId, Input)>);
impl<'a> Iterator for Iter<'a> {
    type Item = (&'static DefPath, &'a Input);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(id, input)| (id.path(), input))
    }
}
impl<'a> IntoIterator for &'a Inputs {
    type Item = (&'static DefPath, &'a Input);
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
