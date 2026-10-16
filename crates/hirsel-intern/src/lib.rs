//! Session-local symbol interning (LANG-4). Stable identities store spellings instead.
pub mod work;
use std::collections::HashMap;
use std::sync::Arc;

/// Index into the owning interner (LANG-4); never persist this as a definition ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Symbol(pub u32);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Rows {
    strings: Vec<String>,
    indices: HashMap<String, Symbol>,
}
/// Session-local spellings with immutable snapshots (LANG-4).
#[derive(Clone, Default, Eq, PartialEq)]
pub struct Interner {
    rows: Arc<Rows>,
}
impl std::fmt::Debug for Interner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Interner")
            .field("strings", &self.rows.strings)
            .field("indices", &self.rows.indices)
            .finish()
    }
}
impl Interner {
    pub fn intern(&mut self, text: &str) -> Symbol {
        if let Some(&symbol) = self.rows.indices.get(text) {
            return symbol;
        }
        let symbol =
            Symbol(u32::try_from(self.rows.strings.len()).expect("symbol arena exceeds u32"));
        if Arc::strong_count(&self.rows) > 1 {
            work::count_by(work::Counter::CopiedSymbols, self.rows.strings.len() as u64);
        }
        let rows = Arc::make_mut(&mut self.rows);
        rows.strings.push(text.to_owned());
        rows.indices.insert(text.to_owned(), symbol);
        symbol
    }
    pub fn resolve(&self, symbol: Symbol) -> Option<&str> {
        self.rows.strings.get(symbol.0 as usize).map(String::as_str)
    }
    pub fn len(&self) -> usize {
        self.rows.strings.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.strings.is_empty()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_keep_their_spelling_rows_when_the_session_extends() {
        let mut names = Interner::default();
        let first = names.intern("first");
        let snapshot = names.clone();
        let before = work::read(work::Counter::CopiedSymbols);
        assert_eq!(names.intern("first"), first);
        assert_eq!(work::read(work::Counter::CopiedSymbols), before);
        let second = names.intern("second");
        assert_eq!(work::read(work::Counter::CopiedSymbols) - before, 1);
        assert_eq!(snapshot.resolve(first), Some("first"));
        assert_eq!(snapshot.resolve(second), None);
        assert_eq!(names.resolve(second), Some("second"));
        assert_ne!(snapshot, names);
    }
}
