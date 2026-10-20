//! `GeneratedActualKey`: the CT-5 instance equivalence (design §1a, rule 4).
//! It is main's key exactly, the CT-3 closed canonical spelling of each actual
//! (modes and callable parameter names are not spelled, sizes are decimal),
//! interned once so selection compares ids.
use crate::*;
use hirsel_intern::Symbol;
use std::sync::Arc;

/// Separates actuals in the interned key; no CT-3 spelling contains it, and
/// it orders before every character, so keys order as main's spelling lists.
const SEPARATOR: char = '\u{0}';

/// The interned closed spelling of a generated declaration's actuals. Eq and
/// Hash compare ids; Ord compares the spellings, as main's `Vec<String>` keys.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GeneratedActualKey(Symbol);

impl GeneratedActualKey {
    /// Each actual's spelling, in argument order: the generated DefPath's
    /// hash input (`instance$<H("hirsel.generated-instance.v1", spellings)>`).
    pub fn spellings(self) -> Vec<&'static str> {
        let text = self.0.as_str();
        if text.is_empty() {
            return vec![];
        }
        text[1..].split(SEPARATOR).collect()
    }
}
impl std::fmt::Debug for GeneratedActualKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("GeneratedActualKey")
            .field(&self.spellings())
            .finish()
    }
}

/// The key of closed `arguments`; None when an argument still names a
/// parameter. Each type's spelling is memoized per TypeId.
pub fn generated_actual_key(
    t: &TypeInterner,
    arguments: &[GenericArgument],
) -> Option<GeneratedActualKey> {
    let mut key = String::new();
    for argument in arguments {
        key.push(SEPARATOR);
        match argument {
            GenericArgument::Type(ty) => key.push_str(&closed_spelling(t, *ty)?),
            GenericArgument::Size(Size::Constant(n)) => key.push_str(&n.to_string()),
            GenericArgument::Size(Size::Parameter(_)) => return None,
        }
    }
    Some(GeneratedActualKey(Symbol::intern(&key)))
}

/// CT-3 closed spelling of `ty`, or None if it names a parameter.
fn closed_spelling(t: &TypeInterner, ty: TypeId) -> Option<Arc<str>> {
    let compute =
        || (!mentions_parameter(t, ty)).then(|| Arc::from(CanonicalSpelling::closed(t, ty)));
    match t.entry(ty) {
        Some(entry) => entry.actual_key.get_or_init(compute).clone(),
        None => compute(),
    }
}

/// Persisted as the spellings, never as a session id.
impl serde::Serialize for GeneratedActualKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.spellings(), s)
    }
}
impl<'de> serde::Deserialize<'de> for GeneratedActualKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let spellings: Vec<String> = serde::Deserialize::deserialize(d)?;
        let mut key = String::new();
        for spelling in spellings {
            key.push(SEPARATOR);
            key.push_str(&spelling);
        }
        Ok(GeneratedActualKey(Symbol::intern(&key)))
    }
}
