//! A lookup index beside ordered elements. It never supplies iteration order.
use super::{Block, Value};
use crate::library::collection::{KeyedError, data_equal};
use crate::{Error, invalid};
use hirsel_intern::work::{Counter, count};
use std::collections::HashMap;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum KeyKind {
    Map,
    Set,
}
impl KeyKind {
    fn key(self, entry: &Value) -> Result<&Value, Error> {
        if self == Self::Set {
            return Ok(entry);
        }
        let Value::Tuple(pair) = entry else {
            return Err(invalid("invalid map entry"));
        };
        if pair.len() != 2 {
            return Err(invalid("invalid map pair"));
        }
        pair[0].as_ref().ok_or_else(|| invalid("dead map key"))
    }
}
pub(super) struct KeyIndex {
    kind: KeyKind,
    buckets: HashMap<u64, Vec<usize>, hirsel_intern::IdHasher>,
    // A constant can contain a key deeper than the runtime hash limit. A
    // shallow query must still inspect it without introducing a hash failure.
    unhashed: Vec<usize>,
}
impl KeyIndex {
    fn build(elements: &[Option<Value>], kind: KeyKind) -> Option<Self> {
        let mut index = Self {
            kind,
            buckets: HashMap::with_hasher(Default::default()),
            unhashed: vec![],
        };
        for (ordinal, element) in elements.iter().enumerate() {
            count(Counter::InterpKeyedRows);
            let key = kind.key(element.as_ref()?).ok()?;
            match crate::structural::hash(key, 0) {
                Ok(hash) => index.buckets.entry(hash).or_default().push(ordinal),
                Err(_) => index.unhashed.push(ordinal),
            }
        }
        Some(index)
    }
}
impl Block {
    /// Hash the query once, then compare only matching hashes. The returned
    /// hash also serves an append after a miss.
    pub(crate) fn find_key(
        &self,
        key: &Value,
        kind: KeyKind,
    ) -> Result<(Option<usize>, u64), KeyedError> {
        let hash = crate::structural::hash(key, 0).map_err(|error| match error {
            crate::structural::StructuralError::Depth => KeyedError::Depth,
            crate::structural::StructuralError::InvalidValue => {
                KeyedError::Invalid(invalid("key is not key data"))
            }
        })?;
        let index = self
            .0
            .index
            .get_or_init(|| KeyIndex::build(self, kind).map(Box::new));
        let Some(index) = index.as_ref().filter(|index| index.kind == kind) else {
            for (ordinal, element) in self.iter().enumerate() {
                count(Counter::InterpKeyedRows);
                let element = element
                    .as_ref()
                    .ok_or_else(|| invalid("dead collection element"))?;
                if data_equal(kind.key(element)?, key) {
                    return Ok((Some(ordinal), hash));
                }
            }
            return Ok((None, hash));
        };
        let bucket = index.buckets.get(&hash).map(Vec::as_slice).unwrap_or(&[]);
        let mut found = None;
        for &ordinal in bucket.iter().chain(&index.unhashed) {
            count(Counter::InterpKeyedRows);
            let element = self[ordinal]
                .as_ref()
                .ok_or_else(|| invalid("dead collection element"))?;
            if data_equal(kind.key(element)?, key) {
                found = Some(found.map_or(ordinal, |previous: usize| previous.min(ordinal)));
            }
        }
        Ok((found, hash))
    }
    /// Append without invalidating a lookup index whose positions stay fixed.
    pub(crate) fn push_keyed(&mut self, entry: Value, hash: u64, kind: KeyKind) {
        let data = self.make_mut();
        if data
            .index
            .get()
            .is_some_and(|index| index.as_ref().is_some_and(|index| index.kind != kind))
        {
            data.index.take();
        }
        count(Counter::InterpKeyedRows);
        let ordinal = data.elements.len();
        data.elements.push(Some(entry));
        if let Some(Some(index)) = data.index.get_mut() {
            if index.kind == kind {
                index.buckets.entry(hash).or_default().push(ordinal);
            }
        }
    }
    /// Map values may change while their immutable keys keep the index valid.
    pub(crate) fn map_value_mut(&mut self, ordinal: usize) -> Result<&mut Option<Value>, Error> {
        let Some(Some(Value::Tuple(pair))) = self.make_mut().elements.get_mut(ordinal) else {
            return Err(invalid("invalid map entry"));
        };
        if pair.len() != 2 {
            return Err(invalid("invalid map pair"));
        }
        Ok(&mut pair[1])
    }
}
