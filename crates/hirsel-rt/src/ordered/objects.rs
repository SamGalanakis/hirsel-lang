//! Native words are representation, not identity. This table relates typed
//! native handles to creator-path/ordinal identities under the scheduler role.
//! Raw legacy scans conservatively union every binding of an ambiguous word.

use std::collections::{BTreeMap, BTreeSet};

use super::capture::Captures;
use super::footprint::{Carries, Footprint, Held, Item, ObjectId, TypeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Kind {
    Atomic,
    Mutex,
    Sender,
    Receiver,
    Task,
    TaskSet,
    Scope,
    Capability,
}

impl Kind {
    pub fn code(self) -> u64 {
        match self {
            Self::Atomic => 1,
            Self::Mutex => 2,
            Self::Sender => 3,
            Self::Receiver => 4,
            Self::Task => 5,
            Self::TaskSet => 6,
            Self::Scope => 7,
            Self::Capability => 8,
        }
    }
    pub fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            1 => Self::Atomic,
            2 => Self::Mutex,
            3 => Self::Sender,
            4 => Self::Receiver,
            5 => Self::Task,
            6 => Self::TaskSet,
            7 => Self::Scope,
            8 => Self::Capability,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Binding {
    object: Option<ObjectId>,
    obtains: BTreeSet<TypeId>,
    unknown: bool,
}

/// Only a successful scan and complete bindings can certify a held set. A
/// binding remains for the run's lifetime: a late speculative capture must not
/// turn a closed handle into an empty footprint. Native generations separate
/// reused slots; no address or table index enters the canonical identity.
#[derive(Default)]
pub struct Registry {
    handles: BTreeMap<u64, BTreeMap<Kind, Binding>>,
    pub carries: Carries,
    families: std::sync::Arc<BTreeMap<u64, BTreeSet<u32>>>,
}

impl Registry {
    /// Read-only canonical identity lookup for an initialized-value decoder.
    /// Typed families disambiguate native words that coincide across tables.
    pub fn identity(&self, kind: Kind, word: u64) -> Option<&ObjectId> {
        self.handles.get(&word)?.get(&kind)?.object.as_ref()
    }

    /// Install the admitted native type graph before dispatch. Runtime object
    /// creation ABIs sometimes erase their generic handle type; register such
    /// an object under every linked type of its known family. This overstates
    /// conflicts and never excludes a possible object from a type closure.
    pub fn install_types(&mut self, types: super::capture::Types) {
        self.carries.install_types(types.definitions);
        self.families = types.families;
        for bindings in self.handles.values() {
            for (&kind, binding) in bindings {
                if let Some(object) = &binding.object {
                    for &ty in self.families.get(&kind.code()).into_iter().flatten() {
                        self.carries.register(ty, object.clone());
                    }
                }
            }
        }
    }
    /// Register creation, or another endpoint alias of an existing object.
    /// `ty` is the linked handle type, not its result/message payload type.
    /// `obtains` names the handle types its payload can expose in this turn.
    pub fn bind(
        &mut self,
        kind: Kind,
        word: u64,
        object: ObjectId,
        ty: TypeId,
        obtains: BTreeSet<TypeId>,
    ) {
        self.carries.register(ty, object.clone());
        self.bind_erased(kind, word, object, obtains);
    }

    /// Native creation ABIs may erase their generic handle type. Register the
    /// object under all admitted types of its known family, without inventing
    /// a layout index for the erased type.
    pub fn bind_erased(
        &mut self,
        kind: Kind,
        word: u64,
        object: ObjectId,
        obtains: BTreeSet<TypeId>,
    ) {
        for &ty in self.families.get(&kind.code()).into_iter().flatten() {
            self.carries.register(ty, object.clone());
        }
        self.insert(
            kind,
            word,
            Binding {
                object: Some(object),
                obtains,
                unknown: false,
            },
        );
    }

    /// A duplicated endpoint keeps the source's logical object and closure.
    pub fn alias(&mut self, kind: Kind, source: u64, alias: u64) {
        let binding = self
            .handles
            .get(&source)
            .and_then(|kinds| kinds.get(&kind))
            .expect("an admitted source handle")
            .clone();
        self.insert(kind, alias, binding);
    }

    /// An explicitly classified handle which cannot name or obtain an E
    /// object (for example a scope token). Unknown handles never use this path.
    pub fn inert(&mut self, kind: Kind, word: u64) {
        self.insert(
            kind,
            word,
            Binding {
                object: None,
                obtains: BTreeSet::new(),
                unknown: false,
            },
        );
    }

    /// Register a foreign or otherwise unclassified handle. Its word may
    /// collide with a known family; the known binding must not hide this one.
    pub fn unknown(&mut self, kind: Kind, word: u64) {
        self.insert(
            kind,
            word,
            Binding {
                object: None,
                obtains: BTreeSet::new(),
                unknown: true,
            },
        );
    }

    fn insert(&mut self, kind: Kind, word: u64, binding: Binding) {
        let row = self.handles.entry(word).or_default();
        if let Some(old) = row.get(&kind) {
            assert_eq!(old, &binding, "native generation reused for another object");
        } else {
            row.insert(kind, binding);
        }
    }

    /// Capture scans in descriptor14 have no family operand. An atomic and a
    /// task can have the same word: include both, never select one by guessing.
    pub fn captured(&self, captures: &Captures) -> Held {
        let mut held = Held::empty();
        held.unknown = captures.unknown;
        for handle in &captures.typed {
            if let Some(kind) = Kind::from_code(handle.kind) {
                self.obtained(&mut held, kind, handle.word);
                held.obtains.extend(handle.obtains);
            } else {
                held.unknown = true;
            }
        }
        for word in &captures.handles {
            match self.handles.get(word) {
                Some(bindings) => {
                    for binding in bindings.values() {
                        Self::include(&mut held, binding);
                    }
                }
                None => held.unknown = true,
            }
        }
        held
    }

    /// A boundary obtains a handle in the same turn (TaskSetTake, or a
    /// receiver's leftover finalizer). Add it before any following E operation.
    pub fn obtained(&self, held: &mut Held, kind: Kind, word: u64) {
        if let Some(binding) = self.handles.get(&word).and_then(|row| row.get(&kind)) {
            Self::include(held, binding);
        } else {
            held.unknown = true;
        }
    }

    fn include(held: &mut Held, binding: &Binding) {
        held.unknown |= binding.unknown;
        held.objects.extend(binding.object.iter().cloned());
        held.obtains.extend(&binding.obtains);
    }

    /// Resolve an operation's concrete read or write, which is always typed
    /// by its runtime entry. Missing metadata parks the operation at the head.
    pub fn footprint(&self, kind: Kind, word: u64, write: bool) -> Option<Footprint> {
        let object = self.handles.get(&word)?.get(&kind)?.object.clone()?;
        let mut footprint = Footprint::default();
        let items = if write {
            &mut footprint.writes
        } else {
            &mut footprint.reads
        };
        items.insert(Item::Object(object));
        Some(footprint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(ordinal: u64) -> ObjectId {
        ObjectId {
            creator: vec![1],
            ordinal,
        }
    }

    #[test]
    fn raw_word_collisions_union_both_families_but_operations_resolve_exactly() {
        let mut registry = Registry::default();
        registry.bind(Kind::Atomic, 17, object(0), 1, BTreeSet::new());
        registry.bind(Kind::Task, 17, object(1), 2, BTreeSet::from([3]));
        let held = registry.captured(&Captures {
            handles: BTreeSet::from([17]),
            typed: BTreeSet::new(),
            unknown: false,
            roots: false,
        });
        assert_eq!(held.objects, BTreeSet::from([object(0), object(1)]));
        assert_eq!(held.obtains, BTreeSet::from([3]));
        assert!(!held.unknown);
        assert_eq!(
            registry.footprint(Kind::Atomic, 17, true).unwrap().writes,
            BTreeSet::from([Item::Object(object(0))])
        );
    }

    #[test]
    fn new_objects_join_obtainable_type_closure_even_after_capture() {
        let mut registry = Registry::default();
        registry.bind(Kind::Receiver, 31, object(0), 1, BTreeSet::from([2]));
        registry.carries.define(2, BTreeSet::from([3]));
        registry.carries.define(3, BTreeSet::from([2]));
        let held = registry.captured(&Captures {
            handles: BTreeSet::from([31]),
            typed: BTreeSet::new(),
            unknown: false,
            roots: false,
        });
        let tasks = BTreeMap::from([(1, held)]);
        registry.bind(Kind::Atomic, 42, object(1), 3, BTreeSet::new());
        let operation = registry.footprint(Kind::Atomic, 42, false).unwrap();
        assert!(!registry.carries.disjoint(1, &tasks, &operation));
    }

    #[test]
    fn only_explicitly_inert_handles_certify_empty_captures() {
        let mut registry = Registry::default();
        let captures = Captures {
            handles: BTreeSet::from([5]),
            typed: BTreeSet::new(),
            unknown: false,
            roots: false,
        };
        assert!(registry.captured(&captures).unknown);
        registry.inert(Kind::Scope, 5);
        let mut held = registry.captured(&captures);
        assert!(!held.unknown);
        assert!(held.objects.is_empty());
        registry.obtained(&mut held, Kind::Task, 5);
        assert!(held.unknown);
        registry.unknown(Kind::Capability, 5);
        assert!(registry.captured(&captures).unknown);
    }
}
