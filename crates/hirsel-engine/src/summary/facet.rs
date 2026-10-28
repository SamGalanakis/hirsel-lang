//! Exact comparison of the settled Summary projection. Positions remain in
//! the published source product; this projection grants no body/V authority.
use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// The memo graph observes the value owned by its existing typed memo table.
/// Evicting that owner makes equality unavailable rather than retaining a body
/// or an unadmitted history in a second table.
#[derive(Clone)]
pub(crate) enum SettledSummary {
    Published(Weak<PublishedSummary>, Weak<TypeInterner>),
    Checked(Weak<CheckedSummary>, Weak<TypeInterner>),
    #[cfg(test)]
    Test(ContentHash),
}
impl SettledSummary {
    pub(crate) fn published(value: &Arc<PublishedSummary>, types: &Arc<TypeInterner>) -> Self {
        Self::Published(Arc::downgrade(value), Arc::downgrade(types))
    }
    pub(crate) fn checked(value: &Arc<CheckedSummary>, types: &Arc<TypeInterner>) -> Self {
        Self::Checked(Arc::downgrade(value), Arc::downgrade(types))
    }
    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Published(a, at), Self::Published(b, bt)) => {
                let (Some(a), Some(b), Some(at), Some(bt)) =
                    (a.upgrade(), b.upgrade(), at.upgrade(), bt.upgrade())
                else {
                    return false;
                };
                (Arc::ptr_eq(&a, &b) && Arc::ptr_eq(&at, &bt))
                    || SummaryView::from(&*a).equivalent(&at, SummaryView::from(&*b), &bt)
            }
            (Self::Checked(a, at), Self::Checked(b, bt)) => {
                let (Some(a), Some(b), Some(at), Some(bt)) =
                    (a.upgrade(), b.upgrade(), at.upgrade(), bt.upgrade())
                else {
                    return false;
                };
                (Arc::ptr_eq(&a, &b) && Arc::ptr_eq(&at, &bt))
                    || SummaryView::from(&*a).equivalent(&at, SummaryView::from(&*b), &bt)
            }
            #[cfg(test)]
            (Self::Test(a), Self::Test(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SummaryView<'a> {
    pub summary: &'a DefinitionSummary,
    pub growth: &'a ParameterGrowth,
    pub checked: Option<&'a CheckedSummary>,
}
impl<'a> From<&'a PublishedSummary> for SummaryView<'a> {
    fn from(value: &'a PublishedSummary) -> Self {
        Self {
            summary: &value.summary,
            growth: &value.growth,
            checked: value.checked.as_deref(),
        }
    }
}
impl<'a> From<&'a CheckedSummary> for SummaryView<'a> {
    fn from(value: &'a CheckedSummary) -> Self {
        Self {
            summary: &value.summary,
            growth: &value.growth,
            checked: None,
        }
    }
}
impl serde::Serialize for SummaryView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            (
                SemanticDefinition(self.summary),
                SemanticGrowth(self.growth),
            ),
            self.checked
                .map(|c| (SemanticDefinition(&c.summary), SemanticGrowth(&c.growth))),
        )
            .serialize(serializer)
    }
}
struct SemanticDefinition<'a>(&'a DefinitionSummary);
impl serde::Serialize for SemanticDefinition<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let DefinitionSummary {
            owner,
            frames,
            calls,
            staged,
        } = self.0;
        (owner, frames, SemanticCalls(calls), staged).serialize(serializer)
    }
}
struct SemanticCalls<'a>(&'a [SummaryCall]);
impl serde::Serialize for SemanticCalls<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for call in self.0 {
            let SummaryCall {
                callee,
                binder,
                arguments,
                owning_frame,
                target_frame,
                class,
                site: _,
            } = call;
            seq.serialize_element(&(
                callee,
                binder,
                arguments,
                owning_frame,
                target_frame,
                class,
                Span::default(),
            ))?;
        }
        seq.end()
    }
}
struct SemanticGrowth<'a>(&'a ParameterGrowth);
impl serde::Serialize for SemanticGrowth<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.0.edges.len()))?;
        for (from, to, grows, _site) in &self.0.edges {
            seq.serialize_element(&(from, to, grows, Span::default()))?;
        }
        seq.end()
    }
}
impl SummaryView<'_> {
    /// Compare the existing normalized projection without encoding or hashing.
    /// Type ids are shortcuts only within their own immutable interner owner.
    pub fn equivalent(
        self,
        types: &TypeInterner,
        other: SummaryView<'_>,
        other_types: &TypeInterner,
    ) -> bool {
        let mut equal = TypeEquality {
            left: types,
            right: other_types,
            forward: HashMap::new(),
            reverse: HashMap::new(),
            force_walk: false,
            used_structural: false,
        };
        if !equal.view(self, other) {
            return false;
        }
        // Identical handles need no walk unless this comparison also paired
        // different ids from the same pool. In that rare case, include all
        // roots in the bijection so shortcuts cannot hide changed sharing.
        if std::ptr::eq(types, other_types) && equal.used_structural {
            equal.forward.clear();
            equal.reverse.clear();
            equal.force_walk = true;
            equal.view(self, other)
        } else {
            true
        }
    }
}

fn growth(a: &ParameterGrowth, b: &ParameterGrowth) -> bool {
    a.edges.len() == b.edges.len()
        && a.edges.iter().zip(&b.edges).all(|(a, b)| {
            let (from, to, grows, _site) = a;
            let (other_from, other_to, other_grows, _other_site) = b;
            (from, to, grows) == (other_from, other_to, other_grows)
        })
}

struct TypeEquality<'a, S = std::collections::hash_map::RandomState> {
    left: &'a TypeInterner,
    right: &'a TypeInterner,
    // A bijection preserves canonical graph sharing, including recursive
    // components. A hash collision only changes lookup cost.
    forward: HashMap<TypeId, TypeId, S>,
    reverse: HashMap<TypeId, TypeId, S>,
    force_walk: bool,
    used_structural: bool,
}
impl<S: std::hash::BuildHasher> TypeEquality<'_, S> {
    fn view(&mut self, a: SummaryView<'_>, b: SummaryView<'_>) -> bool {
        self.summary(a.summary, b.summary)
            && growth(a.growth, b.growth)
            && match (a.checked, b.checked) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    self.summary(&a.summary, &b.summary) && growth(&a.growth, &b.growth)
                }
                _ => false,
            }
    }
    fn summary(&mut self, a: &DefinitionSummary, b: &DefinitionSummary) -> bool {
        let DefinitionSummary {
            owner,
            frames,
            calls,
            staged,
        } = a;
        let DefinitionSummary {
            owner: other_owner,
            frames: other_frames,
            calls: other_calls,
            staged: other_staged,
        } = b;
        owner == other_owner
            && frames == other_frames
            && staged == other_staged
            && calls.len() == other_calls.len()
            && calls.iter().zip(other_calls).all(|(a, b)| {
                let SummaryCall {
                    callee,
                    binder,
                    arguments,
                    owning_frame,
                    target_frame,
                    class,
                    site: _,
                } = a;
                let SummaryCall {
                    callee: other_callee,
                    binder: other_binder,
                    arguments: other_arguments,
                    owning_frame: other_owning_frame,
                    target_frame: other_target_frame,
                    class: other_class,
                    site: _,
                } = b;
                (callee, binder, owning_frame, target_frame, class)
                    == (
                        other_callee,
                        other_binder,
                        other_owning_frame,
                        other_target_frame,
                        other_class,
                    )
                    && arguments.len() == other_arguments.len()
                    && arguments
                        .iter()
                        .zip(other_arguments)
                        .all(|(a, b)| match (a, b) {
                            (GenericArgument::Type(a), GenericArgument::Type(b)) => self.ty(*a, *b),
                            (GenericArgument::Size(a), GenericArgument::Size(b)) => a == b,
                            _ => false,
                        })
            })
    }

    fn ty(&mut self, a: TypeId, b: TypeId) -> bool {
        if !self.force_walk && std::ptr::eq(self.left, self.right) && a == b {
            return self.left.get(a).is_some();
        }
        self.used_structural = true;
        let mut pending = vec![(a, b)];
        while let Some((a, b)) = pending.pop() {
            if let Some(previous) = self.forward.get(&a) {
                if *previous != b {
                    return false;
                }
                continue;
            }
            if self.reverse.contains_key(&b) {
                return false;
            }
            let (Some(left), Some(right)) = (self.left.get(a), self.right.get(b)) else {
                return false;
            };
            let mut left_row = left.clone();
            let mut right_row = right.clone();
            left_row.kind = left.kind.map_edges(|_| TypeId::DANGLING);
            right_row.kind = right.kind.map_edges(|_| TypeId::DANGLING);
            if left_row != right_row {
                return false;
            }
            self.forward.insert(a, b);
            self.reverse.insert(b, a);
            pending.extend(
                left.kind
                    .graph_edges()
                    .into_iter()
                    .zip(right.kind.graph_edges())
                    .map(|(a, b)| (a.ty, b.ty)),
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasherDefault, Hasher};
    #[derive(Default)]
    struct Collision;
    impl Hasher for Collision {
        fn finish(&self) -> u64 {
            0
        }
        fn write(&mut self, _: &[u8]) {}
    }
    #[test]
    fn foreign_type_graph_equality_checks_complete_rows_under_forced_collisions() {
        let a = TypeInterner::new(None);
        let b = TypeInterner::new(None);
        let a_unit = a.intern(hirsel_types::Type::unit());
        let b_unit = b.intern(hirsel_types::Type::unit());
        let a_box = a.intern(hirsel_types::Type::with_kind(TypeKind::Box(a_unit)));
        let b_box = b.intern(hirsel_types::Type::with_kind(TypeKind::Box(b_unit)));
        let mut equal = TypeEquality {
            left: &a,
            right: &b,
            forward: HashMap::<_, _, BuildHasherDefault<Collision>>::default(),
            reverse: HashMap::<_, _, BuildHasherDefault<Collision>>::default(),
            force_walk: false,
            used_structural: false,
        };
        assert!(equal.ty(a_unit, b_unit));
        assert!(equal.ty(a_box, b_box));
        assert!(!equal.ty(a_box, b_unit));
    }
}
