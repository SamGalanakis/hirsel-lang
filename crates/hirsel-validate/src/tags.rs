//! Active cases are finite flow facts; absence means any case is possible.
use super::super::flow_origins::EnvironmentField as EnvField;
use super::*;
use hirsel_types::{BuiltinType, TypeId, TypeInterner};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Field {
    Field(u32),
    Variant(u32, u32),
    Index(Option<u32>),
}
fn normalized(path: &[Field]) -> Vec<EnvField> {
    path.iter()
        .map(|f| match f {
            Field::Field(n) => EnvField::Field(*n),
            Field::Variant(v, n) => EnvField::Variant(*v, *n),
            Field::Index(_) => EnvField::Index,
        })
        .collect()
}
fn indices(path: &[Field]) -> BTreeSet<Carrier> {
    path.iter()
        .filter_map(|f| {
            if let Field::Index(Some(root)) = f {
                Some(Carrier {
                    root: *root,
                    path: vec![],
                })
            } else {
                None
            }
        })
        .collect()
}
fn selected(source: &Carrier, fact: &Carrier) -> bool {
    source.root == fact.root
        && source.path.len() <= fact.path.len()
        && source
            .path
            .iter()
            .zip(&fact.path)
            .all(|(a, b)| a == b || matches!((a, b), (Field::Index(_), Field::Index(None))))
}
fn proven<'a>(facts: &'a Facts, key: &Carrier) -> Option<&'a Fact> {
    facts.get(key).or_else(|| {
        facts
            .iter()
            .find(|(candidate, _)| {
                candidate.path.len() == key.path.len() && selected(key, candidate)
            })
            .map(|(_, fact)| fact)
    })
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct Carrier {
    root: u32,
    path: Vec<Field>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Fact {
    cases: BTreeSet<u32>,
    dependencies: BTreeSet<Carrier>,
}
pub(super) type Facts = BTreeMap<Carrier, Fact>;
pub(crate) type Parts = BTreeMap<Vec<Field>, Fact>;
fn path<'a>(projections: impl IntoIterator<Item = &'a Projection>) -> Vec<Field> {
    projections
        .into_iter()
        .map(|p| match p {
            Projection::Field(f) => Field::Field(*f),
            Projection::VariantField { variant, field } => Field::Variant(*variant, *field),
            Projection::Index(root) => Field::Index(Some(root.0)),
        })
        .collect()
}
fn carrier(place: &Place) -> Carrier {
    Carrier {
        root: place.root.0,
        path: path(&place.projection),
    }
}
fn contains(owner: &Carrier, child: &Carrier) -> bool {
    owner.root == child.root
        && owner.path.len() <= child.path.len()
        && owner
            .path
            .iter()
            .zip(&child.path)
            .all(|(a, b)| a == b || matches!((a, b), (Field::Index(_), Field::Index(_))))
}
pub(super) fn join(target: &mut Facts, incoming: &Facts) {
    target.retain(|key, fact| {
        if let Some(other) = incoming.get(key) {
            fact.cases.extend(&other.cases);
            fact.dependencies.extend(other.dependencies.iter().cloned());
            true
        } else {
            false
        }
    });
}
pub(crate) fn join_parts(target: &mut Parts, incoming: &Parts) {
    target.retain(|key, fact| {
        if let Some(other) = incoming.get(key) {
            fact.cases.extend(&other.cases);
            fact.dependencies.extend(other.dependencies.iter().cloned());
            true
        } else {
            false
        }
    });
}
impl Checker<'_, '_> {
    pub(super) fn tag_parts(&self, state: &State, value: &Operand) -> Parts {
        let source = carrier(&self.body.places[value.place.0 as usize]);
        state
            .tags
            .iter()
            .filter(|(key, _)| selected(&source, key))
            .map(|(key, fact)| {
                let mut fact = fact.clone();
                if matches!(value.usage, UseKind::Borrow | UseKind::Mut) {
                    fact.dependencies.insert(key.clone());
                }
                if value.usage == UseKind::Copy {
                    let addresses = indices(&key.path[source.path.len()..]);
                    fact.dependencies.retain(|d| addresses.contains(d));
                }
                (key.path[source.path.len()..].to_vec(), fact)
            })
            .collect()
    }
    pub(super) fn invalidate_tags(&self, state: &mut State, place: PlaceId) {
        let target = carrier(&self.body.places[place.0 as usize]);
        state.tags.retain(|key, fact| {
            !contains(&target, key)
                && !fact
                    .dependencies
                    .iter()
                    .any(|source| contains(&target, source))
        });
    }
    pub(super) fn install_tags(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        parts: Parts,
    ) {
        let target = carrier(&self.body.places[place.0 as usize]);
        self.invalidate_tags(state, place);
        for (part, mut fact) in parts {
            let mut key = target.clone();
            key.path.extend(part);
            if !model.environment_paths.contains(&normalized(&key.path)) {
                continue;
            }
            fact.dependencies.extend(indices(&key.path));
            state.tags.insert(key, fact);
        }
    }
    pub(super) fn constructed_tags(&self, state: &State, value: &Rvalue) -> Parts {
        match value {
            Rvalue::Use(o)
            | Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value: o,
            } => self.tag_parts(state, o),
            Rvalue::Construct {
                constructor,
                fields,
                ..
            } => {
                let mut parts = Parts::new();
                if let Constructor::Variant { variant, .. } = constructor {
                    parts.insert(
                        vec![],
                        Fact {
                            cases: BTreeSet::from([*variant]),
                            dependencies: BTreeSet::new(),
                        },
                    );
                }
                let mut elements: Option<Parts> = None;
                for field in fields {
                    let input = self.tag_parts(state, &field.value);
                    if matches!(
                        constructor,
                        Constructor::Sequence | Constructor::Map | Constructor::Set
                    ) {
                        if let Some(previous) = &mut elements {
                            join_parts(previous, &input);
                        } else {
                            elements = Some(input);
                        }
                    } else {
                        let prefix = match constructor {
                            Constructor::Variant { variant, .. } => {
                                Field::Variant(*variant, field.field)
                            }
                            _ => Field::Field(field.field),
                        };
                        for (mut path, fact) in input {
                            path.insert(0, prefix.clone());
                            parts.insert(path, fact);
                        }
                    }
                }
                if let Some(elements) = elements {
                    for (mut path, fact) in elements {
                        path.insert(0, Field::Index(None));
                        parts.insert(path, fact);
                    }
                }
                parts
            }
            _ => Parts::new(),
        }
    }
    pub(crate) fn packed_carrier(&self, store: &TypeInterner, mut ty: TypeId) -> bool {
        let mut expansions = hirsel_types::NominalExpansions::default();
        loop {
            match store.get(ty).unwrap().kind.clone() {
                TypeKind::Builtin {
                    kind: BuiltinType::Packed,
                    ..
                } => return true,
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    if !expansions
                        .enter(ty, 0, store)
                        .unwrap_or_else(|error| self.proof_failure(error, false))
                    {
                        return false;
                    }
                    let Some(signature) = self.signatures.get(definition) else {
                        return false;
                    };
                    let SignatureKind::Type(declaration) = &signature.kind else {
                        return false;
                    };
                    let Some(root) = declaration.representation else {
                        return false;
                    };
                    let next = if arguments.is_empty() {
                        Ok(root)
                    } else {
                        hirsel_types::substitute(
                            self.t,
                            self.memo,
                            root,
                            &hirsel_types::Substitution {
                                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                                roles: &signature.metadata.roles,
                                arguments: &arguments,
                                authorities: &vec![
                                    hirsel_types::AuthoritySummary::Unknown;
                                    arguments.len()
                                ],
                            },
                        )
                    };
                    let Ok(next) = next else { return false };
                    ty = next;
                }
                _ => return false,
            }
        }
    }
    pub(super) fn active_tag(&mut self, state: &State, place: PlaceId, report: bool) -> bool {
        let access = self.body.places[place.0 as usize].clone();
        let store = self.t;
        let mut valid = true;
        for (depth, projection) in access.projection.iter().enumerate() {
            let Projection::VariantField { variant, .. } = projection else {
                continue;
            };
            let mut prefix = access.clone();
            prefix.projection.truncate(depth);
            let Some(ty) = self.projection_type(store, &prefix, |_, field| {
                Some(Origin {
                    root: OriginRoot::Sibling(field),
                    fields: vec![].into(),
                })
            }) else {
                valid = false;
                continue;
            };
            if self.packed_carrier(store, ty) {
                continue;
            }
            let single = matches!(self.layout(store, ty), Some(TypeKind::Variant(v)) if v.len() == 1 && *variant == 0);
            if !single
                && !proven(&state.tags, &carrier(&prefix))
                    .is_some_and(|f| f.cases == BTreeSet::from([*variant]))
            {
                valid = false;
            }
        }
        if !valid {
            self.flow_error(report, ViolationKind::InvalidProjection(place));
        }
        valid
    }
    pub(super) fn tag_edge(&mut self, state: &mut State, source: BlockId, target: BlockId) -> bool {
        if let Terminator::Match { table, from } = self.body.blocks[source.0 as usize].terminator {
            let table = &self.body.matches[table.0 as usize];
            let mut alternatives: Option<super::shared::Shared<Facts>> = None;
            for arm in table
                .arms
                .iter()
                .skip(from as usize)
                .filter(|a| a.guard.unwrap_or(a.target) == target)
            {
                let mut facts = state.tags.clone();
                let mut pending = vec![arm.pattern];
                while let Some(id) = pending.pop() {
                    let pattern = &table.patterns[id.0 as usize];
                    let case = match pattern.kind {
                        MatchPatternKind::Variant { variant, .. } => Some(variant),
                        MatchPatternKind::Literal(Constant::Bool(b)) => Some(u32::from(b)),
                        _ => None,
                    };
                    if let Some(case) = case {
                        let key = carrier(&self.body.places[pattern.place.0 as usize]);
                        let mut dependencies = proven(&facts, &key)
                            .map(|f| f.dependencies.clone())
                            .unwrap_or_default();
                        dependencies.extend(indices(&key.path));
                        facts.insert(
                            key,
                            Fact {
                                cases: BTreeSet::from([case]),
                                dependencies,
                            },
                        );
                    }
                    pending.extend(pattern.kind.fields().iter().map(|(_, child)| *child));
                }
                if let Some(previous) = &mut alternatives {
                    join(previous, &facts);
                } else {
                    alternatives = Some(facts);
                }
            }
            if let Some(facts) = alternatives {
                state.tags = facts;
            }
            return true;
        }
        let Terminator::Switch {
            value,
            arms,
            otherwise,
        } = self.body.blocks[source.0 as usize].terminator.clone()
        else {
            return true;
        };
        let store = self.t;
        let count = match self.layout(store, self.place_type(value.place)) {
            Some(TypeKind::Variant(variants)) => variants.len(),
            Some(TypeKind::Primitive(hirsel_types::Primitive::Bool)) => 2,
            _ => return true,
        };
        let mut cases: BTreeSet<u32> = arms.iter().filter(|a| a.1 == target).map(|a| a.0).collect();
        if target == otherwise {
            cases.extend((0..count as u32).filter(|v| !arms.iter().any(|a| a.0 == *v)));
        }
        let key = carrier(&self.body.places[value.place.0 as usize]);
        if let Some(known) = proven(&state.tags, &key) {
            let selected = cases.clone();
            cases.retain(|case| known.cases.contains(case));
            // Contradiction permits only the empty unreachable sentinel to
            // escape replay. Audit executable arms without the old tag proof,
            // just as we audit an executable exhaustive default.
            let sentinel = self.body.blocks[target.0 as usize].instructions.is_empty()
                && matches!(
                    self.body.blocks[target.0 as usize].terminator,
                    Terminator::Unreachable
                );
            if cases.is_empty() {
                if sentinel {
                    return false;
                }
                state.audit = true;
                // Replay the hypothetical arm under its own discriminator,
                // independent of the contradictory predecessor proof.
                cases = selected;
            }
        }
        let mut dependencies = proven(&state.tags, &key)
            .map(|f| f.dependencies.clone())
            .unwrap_or_default();
        dependencies.extend(indices(&key.path));
        if cases.is_empty() {
            state.tags.remove(&key);
        } else {
            state.tags.insert(
                key,
                Fact {
                    cases,
                    dependencies,
                },
            );
        }
        true
    }
}

pub(super) fn capture(state: &mut State, root: u32, parts: &Parts) {
    for (path, fact) in parts {
        // Index identities belong to the caller frame. A copied evaluated
        // element is independent, but borrowed address proofs cannot cross it.
        if path.iter().any(|f| matches!(f, Field::Index(_)))
            || fact
                .dependencies
                .iter()
                .any(|d| d.path.iter().any(|f| matches!(f, Field::Index(_))))
        {
            continue;
        }
        let mut fact = fact.clone();
        fact.dependencies.clear();
        state.tags.insert(
            Carrier {
                root,
                path: path.clone(),
            },
            fact,
        );
    }
}
