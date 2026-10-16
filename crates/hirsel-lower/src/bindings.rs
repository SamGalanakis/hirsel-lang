//! Scoped storage names have one primary stack per exact key. The declaration
//! sequence records only keys needed to undo temporary guard bindings.
use hirsel_tir::PlaceId;
use hirsel_types::{NameSite, ResolvedName};
use std::collections::BTreeMap;
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum Name {
    Local(hirsel_types::LocalId),
    Parameter(hirsel_types::ParamId),
    Definition(hirsel_syntax::DefPath),
    Variant(hirsel_types::VariantId),
    Size(hirsel_types::SizeParameterRef),
    Type(hirsel_types::TypeParameterRef),
    Primitive(u8),
}
impl Name {
    fn of(name: &ResolvedName) -> Self {
        match name {
            ResolvedName::Local(id) => Self::Local(*id),
            ResolvedName::Parameter(id) => Self::Parameter(*id),
            ResolvedName::Definition(path) => Self::Definition(path.clone()),
            ResolvedName::Variant(id) => Self::Variant(*id),
            ResolvedName::Size(id) => Self::Size(*id),
            ResolvedName::TypeParameter(id) => Self::Type(*id),
            ResolvedName::Primitive(kind) => Self::Primitive(*kind as u8),
        }
    }
}
#[derive(Default)]
pub(super) struct Bindings {
    names: BTreeMap<(NameSite, Name), Vec<PlaceId>>,
    declarations: Vec<(NameSite, Name)>,
}
impl Bindings {
    pub fn push(&mut self, (owner, name, place): (NameSite, ResolvedName, PlaceId)) {
        let key = (owner, Name::of(&name));
        self.names.entry(key.clone()).or_default().push(place);
        self.declarations.push(key);
    }
    pub fn len(&self) -> usize {
        self.declarations.len()
    }
    pub fn truncate(&mut self, mark: usize) {
        while self.declarations.len() > mark {
            let key = self.declarations.pop().unwrap();
            let rows = self.names.get_mut(&key).unwrap();
            rows.pop();
            if rows.is_empty() {
                self.names.remove(&key);
            }
        }
    }
    pub fn get(&self, owner: NameSite, name: &ResolvedName) -> Option<PlaceId> {
        hirsel_intern::work::count(hirsel_intern::work::Counter::LoweringBindingRows);
        self.names.get(&(owner, Name::of(name)))?.last().copied()
    }
    pub fn set(&mut self, owner: NameSite, name: &ResolvedName, place: PlaceId) {
        *self
            .names
            .get_mut(&(owner, Name::of(name)))
            .unwrap()
            .last_mut()
            .unwrap() = place;
    }
}
