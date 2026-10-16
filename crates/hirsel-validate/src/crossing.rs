//! Current owned-part crossing evidence, independent of physical contracts.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Part {
    Field(u32),
    Variant(u32, u32),
    Elements,
}
pub(super) fn path(projections: &hirsel_types::SharedPath<Projection>) -> Vec<Part> {
    projections
        .iter()
        .map(|p| match p {
            Projection::Field(f) => Part::Field(*f),
            Projection::VariantField { variant, field } => Part::Variant(*variant, *field),
            Projection::Index(_) => Part::Elements,
        })
        .collect()
}
#[derive(Clone, Default, Eq, PartialEq)]
pub(crate) struct Parts(BTreeMap<Vec<Part>, bool>);
impl Parts {
    pub(super) fn get(&self, path: &[Part]) -> Option<bool> {
        self.0
            .iter()
            .filter(|(p, _)| path.starts_with(p))
            .max_by_key(|(p, _)| p.len())
            .map(|(_, proof)| *proof)
    }
    pub(super) fn set(&mut self, path: &[Part], proof: bool) {
        if proof && path.contains(&Part::Elements) && self.get(path) == Some(false) {
            return;
        }
        self.0.retain(|p, _| !p.starts_with(path));
        if self.get(path) == Some(proof) {
            return;
        }
        self.0.insert(path.to_vec(), proof);
    }
    pub(super) fn project(&self, path: &[Part]) -> Self {
        let mut projected = Self::default();
        if let Some(proof) = self.get(path) {
            projected.set(&[], proof);
        }
        for (p, proof) in self.0.iter().filter(|(p, _)| p.starts_with(path)) {
            // These are explicit facts already proved on the source. Keep
            // stable descendants even when their enclosing owner was shared.
            projected.0.insert(p[path.len()..].to_vec(), *proof);
        }
        projected
    }
    pub(super) fn install(&mut self, path: &[Part], incoming: &Self) {
        self.0.retain(|p, _| !p.starts_with(path));
        for (p, proof) in &incoming.0 {
            let mut installed = path.to_vec();
            installed.extend(p.iter().cloned());
            self.0.insert(installed, *proof);
        }
    }
    /// Finite checked-projection domain, independent of Source flow. Losing a
    /// deep positive proof is conservative; a deep failure remains a failure
    /// of the retained prefix's complete contents.
    pub(super) fn restrict(&mut self, depth: usize) {
        if self.0.keys().all(|path| path.len() <= depth) {
            return;
        }
        let entries = self.0.clone();
        let mut parts = Self::default();
        for (path, proof) in entries.iter().filter(|(path, _)| path.len() <= depth) {
            parts.set(path, *proof);
        }
        for (path, proof) in entries.iter().filter(|(path, _)| path.len() > depth) {
            if !proof {
                parts.set(&path[..depth], false);
            }
        }
        *self = parts;
    }
    pub(crate) fn join(&mut self, other: &Self) {
        if self == other {
            return;
        }
        let mut joined = Self::default();
        for p in self.0.keys().chain(other.0.keys()) {
            let proof = match (self.get(p), other.get(p)) {
                (Some(a), Some(b)) => Some(a && b),
                (Some(false), None) | (None, Some(false)) => Some(false),
                _ => None,
            };
            if let Some(proof) = proof {
                joined.0.insert(p.clone(), proof);
            }
        }
        *self = joined;
    }
}
