//! Retained permissions belong to installed content, independently of access
//! paths and declaration modes. A field replacement cannot expire a sibling.
use super::flow_origins::EnvironmentField;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Default, Eq, PartialEq)]
struct Dependency {
    held: BTreeSet<usize>,
    expired: BTreeSet<usize>,
}
impl Dependency {
    fn join(&mut self, other: &Self) {
        self.held.extend(&other.held);
        self.expired.extend(&other.expired);
    }
}
#[derive(Clone, Default, Eq, PartialEq)]
pub(super) struct Content(BTreeMap<Vec<EnvironmentField>, Dependency>);
impl Content {
    pub fn whole(held: BTreeSet<usize>) -> Self {
        let mut content = Self::default();
        content.add(held);
        content
    }
    pub fn add(&mut self, held: BTreeSet<usize>) {
        if !held.is_empty() {
            self.0.entry(vec![]).or_default().held.extend(held);
        }
    }
    pub fn join(&mut self, other: &Self) {
        for (path, dependency) in &other.0 {
            self.0.entry(path.clone()).or_default().join(dependency);
        }
    }
    pub fn held(&self) -> BTreeSet<usize> {
        self.0
            .values()
            .flat_map(|d| d.held.iter().copied())
            .collect()
    }
    pub fn contains(&self, item: usize) -> bool {
        self.0
            .values()
            .any(|dependency| dependency.held.contains(&item))
    }
    pub fn expired(&self) -> BTreeSet<usize> {
        self.0
            .values()
            .flat_map(|d| d.expired.iter().copied())
            .collect()
    }
    pub fn expire(&mut self, loan: usize) {
        for dependency in self.0.values_mut() {
            if dependency.held.contains(&loan) {
                dependency.expired.insert(loan);
            }
        }
    }
    pub fn project(&self, prefix: &[EnvironmentField]) -> Self {
        let mut projected = Self::default();
        for (path, dependency) in &self.0 {
            if path.starts_with(prefix) {
                projected
                    .0
                    .entry(path[prefix.len()..].to_vec())
                    .or_default()
                    .join(dependency);
            } else if prefix.starts_with(path) {
                projected.0.entry(vec![]).or_default().join(dependency);
            }
        }
        projected
    }
    pub fn insert(
        &mut self,
        prefix: &[EnvironmentField],
        value: &Self,
        paths: &BTreeSet<Vec<EnvironmentField>>,
    ) {
        for (path, dependency) in &value.0 {
            let mut target = prefix.to_vec();
            target.extend(path.iter().cloned());
            // The static body provides a finite universe, including constructor
            // parts. Recursive transports summarize at its nearest prefix.
            while !paths.contains(&target) {
                target.pop();
            }
            self.0.entry(target).or_default().join(dependency);
        }
    }
    pub fn clear(&mut self, prefix: &[EnvironmentField]) {
        // One indexed write cannot prove every element dependency is dead.
        if !prefix.contains(&EnvironmentField::Index) {
            self.0.retain(|path, _| !path.starts_with(prefix));
        }
    }
    pub fn replace(
        &mut self,
        prefix: &[EnvironmentField],
        value: &Self,
        paths: &BTreeSet<Vec<EnvironmentField>>,
    ) {
        self.clear(prefix);
        self.insert(prefix, value, paths);
    }
}
