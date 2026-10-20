//! Private immutable snapshots of finite ordered fact collections.
use hirsel_intern::work::{self, Counter};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
    rc::Rc,
};
#[derive(Clone, Default, Debug)]
pub(super) struct Shared<T>(Rc<T>);
pub(super) trait RowCount {
    fn rows(&self) -> usize;
}
impl<K, V> RowCount for BTreeMap<K, V> {
    fn rows(&self) -> usize {
        self.len()
    }
}
impl<T> RowCount for BTreeSet<T> {
    fn rows(&self) -> usize {
        self.len()
    }
}
impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self(Rc::new(value))
    }
}
impl<T> Shared<T> {
    pub fn shares(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl<T: Default + RowCount> Shared<T> {
    /// Empty this snapshot without copying rows another snapshot still shares.
    pub fn clear(&mut self) {
        if self.0.rows() > 0 {
            *self = T::default().into();
        }
    }
}
impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Clone + RowCount> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        if Rc::strong_count(&self.0) > 1 {
            work::count_by(Counter::ValidateCopiedRows, self.0.rows() as u64);
        }
        Rc::make_mut(&mut self.0)
    }
}
impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        self.shares(other) || *self.0 == *other.0
    }
}
impl<T: Eq> Eq for Shared<T> {}
impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.0.as_ref().into_iter()
    }
}

impl<I, T: FromIterator<I>> FromIterator<I> for Shared<T> {
    fn from_iter<U: IntoIterator<Item = I>>(values: U) -> Self {
        T::from_iter(values).into()
    }
}
