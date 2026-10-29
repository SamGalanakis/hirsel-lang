//! Persisted forms that write session ids as numbers in a table stored beside
//! them (the std image's string and definition tables). Inside an encoding
//! scope an id serializes as its number in a `Numbering`, assigned in
//! first-encounter order, so a deterministic serialization numbers ids the same
//! in every session; inside a decoding scope a number deserializes through a
//! `Source`. Outside any scope ids serialize as their spellings.
use crate::{IdHasher, Symbol};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::thread::LocalKey;

/// The number of each id an encoding met, in first-encounter order.
pub struct Numbering<K> {
    index: HashMap<K, u32, IdHasher>,
    order: Vec<K>,
}
impl<K> Default for Numbering<K> {
    fn default() -> Self {
        Self {
            index: HashMap::default(),
            order: vec![],
        }
    }
}
impl<K: Copy + Eq + Hash> Numbering<K> {
    /// The number of `id`, assigning the next one the first time.
    pub fn number(&mut self, id: K) -> u32 {
        let next = self.order.len() as u32;
        *self.index.entry(id).or_insert_with(|| {
            self.order.push(id);
            next
        })
    }
    /// Every numbered id, by number.
    pub fn ids(&self) -> &[K] {
        &self.order
    }
}

/// The id of each number a decoding reads.
pub trait Source<K>: Send + Sync {
    fn id(&self, number: u32) -> Option<K>;
}

/// One scope of a scope stack: the innermost decides how ids persist.
pub enum Scope<K> {
    Encode(Numbering<K>),
    Decode(Arc<dyn Source<K>>),
    /// Persist complete inline identities, independent of enclosing tables.
    Inline,
}
pub type Scopes<K> = RefCell<Vec<Scope<K>>>;

/// Run `f` with ids of `stack`'s kind serializing as their numbers in `numbering`.
pub fn with_numbering<K: 'static, R>(
    stack: &'static LocalKey<Scopes<K>>,
    numbering: &mut Numbering<K>,
    f: impl FnOnce() -> R,
) -> R {
    stack.with_borrow_mut(|s| s.push(Scope::Encode(std::mem::take(numbering))));
    let popped = Popped { stack, numbering };
    let result = f();
    drop(popped);
    result
}
struct Popped<'a, K: 'static> {
    stack: &'static LocalKey<Scopes<K>>,
    numbering: &'a mut Numbering<K>,
}
impl<K: 'static> Drop for Popped<'_, K> {
    fn drop(&mut self) {
        if let Some(Scope::Encode(numbering)) = self.stack.with_borrow_mut(|s| s.pop()) {
            *self.numbering = numbering;
        }
    }
}

/// Run `f` with ids of `stack`'s kind deserializing from numbers through `source`.
pub fn with_source<K: 'static, R>(
    stack: &'static LocalKey<Scopes<K>>,
    source: Arc<dyn Source<K>>,
    f: impl FnOnce() -> R,
) -> R {
    stack.with_borrow_mut(|s| s.push(Scope::Decode(source)));
    struct Pop<K: 'static>(&'static LocalKey<Scopes<K>>);
    impl<K: 'static> Drop for Pop<K> {
        fn drop(&mut self) {
            self.0.with_borrow_mut(|s| s.pop());
        }
    }
    let _pop = Pop(stack);
    f()
}

/// Run `f` with complete inline identities, masking any enclosing name table.
/// A nested immutable archive owns its own encoding and must not populate or
/// decode through the caller's image or verifier numbering.
pub fn with_inline<K: 'static, R>(stack: &'static LocalKey<Scopes<K>>, f: impl FnOnce() -> R) -> R {
    stack.with_borrow_mut(|s| s.push(Scope::Inline));
    struct Pop<K: 'static>(&'static LocalKey<Scopes<K>>);
    impl<K: 'static> Drop for Pop<K> {
        fn drop(&mut self) {
            self.0.with_borrow_mut(|s| s.pop());
        }
    }
    let _pop = Pop(stack);
    f()
}

/// The number `id` serializes as, if the innermost scope encodes.
pub fn encoded<K: Copy + Eq + Hash + 'static>(
    stack: &'static LocalKey<Scopes<K>>,
    id: K,
) -> Option<u32> {
    stack.with_borrow_mut(|s| match s.last_mut() {
        Some(Scope::Encode(numbering)) => Some(numbering.number(id)),
        _ => None,
    })
}

/// The source numbers deserialize through, if the innermost scope decodes.
pub fn decoding<K: 'static>(stack: &'static LocalKey<Scopes<K>>) -> Option<Arc<dyn Source<K>>> {
    stack.with_borrow(|s| match s.last() {
        Some(Scope::Decode(source)) => Some(source.clone()),
        _ => None,
    })
}

thread_local! {
    static SYMBOLS: Scopes<Symbol> = const { RefCell::new(vec![]) };
}

/// Run `f` with every `Symbol` serializing as its number in `numbering`.
pub fn with_symbol_numbering<R>(numbering: &mut Numbering<Symbol>, f: impl FnOnce() -> R) -> R {
    with_numbering(&SYMBOLS, numbering, f)
}

/// Run `f` with every `Symbol` deserializing from a number through `source`.
pub fn with_symbol_source<R>(source: Arc<dyn Source<Symbol>>, f: impl FnOnce() -> R) -> R {
    with_source(&SYMBOLS, source, f)
}

/// Run `f` with symbols persisted as spellings, even inside a name table.
pub fn with_symbol_spellings<R>(f: impl FnOnce() -> R) -> R {
    with_inline(&SYMBOLS, f)
}

pub(crate) fn symbol_number(symbol: Symbol) -> Option<u32> {
    encoded(&SYMBOLS, symbol)
}

pub(crate) fn symbol_source() -> Option<Arc<dyn Source<Symbol>>> {
    decoding(&SYMBOLS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_scope_restores_numbering_after_unwind() {
        let symbol = Symbol::intern("inline-name");
        let mut numbering = Numbering::default();
        with_symbol_numbering(&mut numbering, || {
            assert_eq!(serde_json::to_string(&symbol).unwrap(), "0");
            let panic = std::panic::catch_unwind(|| {
                with_symbol_spellings(|| {
                    assert_eq!(serde_json::to_string(&symbol).unwrap(), "\"inline-name\"");
                    panic!("leave nested archive");
                })
            });
            assert!(panic.is_err());
            assert_eq!(serde_json::to_string(&symbol).unwrap(), "0");
        });
        assert_eq!(numbering.ids(), [symbol]);
        with_symbol_source(Arc::new(Table(vec![symbol])), || {
            let inline: Symbol =
                with_symbol_spellings(|| serde_json::from_str("\"inline-name\"").unwrap());
            assert_eq!(inline, symbol);
            assert_eq!(serde_json::from_str::<Symbol>("0").unwrap(), symbol);
        });
    }

    struct Table(Vec<Symbol>);
    impl Source<Symbol> for Table {
        fn id(&self, number: u32) -> Option<Symbol> {
            self.0.get(number as usize).copied()
        }
    }

    #[test]
    fn symbols_persist_as_first_encounter_numbers_and_read_back() {
        let words = ["persist.b", "persist.a", "persist.b", "persist.c"].map(Symbol::intern);
        let mut numbering = Numbering::default();
        let bytes =
            with_symbol_numbering(&mut numbering, || serde_json::to_string(&words).unwrap());
        assert_eq!(bytes, "[0,1,0,2]");
        assert_eq!(numbering.ids(), [words[0], words[1], words[3]]);
        let read: Vec<Symbol> =
            with_symbol_source(Arc::new(Table(numbering.ids().to_vec())), || {
                serde_json::from_str(&bytes).unwrap()
            });
        assert_eq!(read, words);
        // Outside a scope a symbol is its spelling again.
        assert_eq!(serde_json::to_string(&words[0]).unwrap(), "\"persist.b\"");
    }

    #[test]
    fn a_number_without_a_symbol_fails_to_decode() {
        let read = with_symbol_source(Arc::new(Table(vec![])), || {
            serde_json::from_str::<Symbol>("7")
        });
        assert!(read.is_err());
    }
}
