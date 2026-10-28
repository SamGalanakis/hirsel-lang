//! Recursive type groups: type declarations of one package whose bodies name
//! each other, such as `type Tree = Leaf | Node(box Entry)` and
//! `type Entry { left: Tree, key: u64, right: Tree }`. Each member's signature
//! reads the others' provisional signatures (`GroupStage`) instead of their
//! published ones, which would wait on it. Membership comes from the written
//! type paths and manifest lookups, never from a signature, so finding a group
//! cannot re-enter one.
use super::*;
use hirsel_syntax::{TypeArgument, TypeBody, TypeExprId, TypeExprKind};
use hirsel_types::{GroupStage, RecursiveGroup};

/// A group's members in DefPath order, the owner included, and each member's
/// `hirsel_graph::scc_keys` key over the members' paths and semantic tokens.
pub(super) struct TypeGroup {
    pub(super) members: Vec<DefPath>,
    keys: BTreeMap<DefPath, ContentHash>,
}

impl Recorder<'_> {
    /// `definition`'s recursive type group, or None outside one: a type that
    /// names only itself is checked as its own recursion, and a cycle through
    /// an alias stays the alias error.
    pub(super) fn type_group(&mut self, definition: &DefPath) -> Option<Arc<TypeGroup>> {
        if let Some(group) = self.groups.get(definition) {
            return group.clone();
        }
        let group = self.find_type_group(definition).map(Arc::new);
        for member in group.iter().flat_map(|g| g.members.iter()) {
            self.groups.insert(member.clone(), group.clone());
        }
        self.groups.insert(definition.clone(), group.clone());
        group
    }
    fn find_type_group(&mut self, definition: &DefPath) -> Option<TypeGroup> {
        self.type_references(definition)?;
        let mut nodes = vec![definition.clone()];
        let mut index = BTreeMap::from([(definition.clone(), 0usize)]);
        let mut succ: Vec<Vec<usize>> = vec![];
        while succ.len() < nodes.len() {
            let node = nodes[succ.len()].clone();
            let targets = self
                .type_references(&node)
                .unwrap_or_default()
                .into_iter()
                .map(|target| {
                    *index.entry(target.clone()).or_insert_with(|| {
                        nodes.push(target);
                        nodes.len() - 1
                    })
                })
                .collect();
            succ.push(targets);
        }
        let component = hirsel_graph::components(&succ)
            .into_iter()
            .find(|c| c.contains(&0))?;
        if component.len() < 2 || component.iter().any(|m| self.is_alias(&nodes[*m])) {
            return None;
        }
        let inside: BTreeSet<usize> = component.iter().copied().collect();
        // Each member's semantic tokens, not its text: layout, comments and
        // analysis annotations (PS-19) never move a member's signature.
        let texts: BTreeMap<usize, Vec<Vec<u8>>> = component
            .iter()
            .map(|m| {
                let tokens = self
                    .engine
                    .items
                    .get(&nodes[*m])
                    .and_then(|items| items.first())
                    .and_then(|syntax| self.engine.semantic_tokens(syntax).ok())
                    .unwrap_or_default();
                (*m, tokens)
            })
            .collect();
        let keys = hirsel_graph::scc_keys(
            &component,
            |m| {
                succ[m]
                    .iter()
                    .copied()
                    .filter(|t| inside.contains(t))
                    .collect()
            },
            |m, writer| {
                writer.bytes(nodes[m].to_string().as_bytes());
                for token in &texts[&m] {
                    writer.bytes(token);
                }
            },
        );
        let mut members: Vec<DefPath> = component.iter().map(|m| nodes[*m].clone()).collect();
        members.sort();
        Some(TypeGroup {
            members,
            keys: keys
                .into_iter()
                .map(|(m, key)| (nodes[m].clone(), key))
                .collect(),
        })
    }
    fn is_alias(&self, definition: &DefPath) -> bool {
        self.engine
            .items
            .get(definition)
            .and_then(|items| items.first())
            .is_some_and(|snapshot| {
                matches!(
                    &snapshot.item.kind,
                    ItemKind::Type {
                        body: TypeBody::Alias(_),
                        ..
                    }
                )
            })
    }
    /// The type declarations of `definition`'s package that its body names,
    /// in source order; None when it is not a type declaration of this
    /// session. A path naming one of its generic parameters names no type.
    fn type_references(&mut self, definition: &DefPath) -> Option<Vec<DefPath>> {
        self.engine.materialize(definition).ok()?;
        let snapshot = self.engine.items.get(definition)?.first()?.clone();
        let item = &snapshot.item;
        let ItemKind::Type { generics, body, .. } = &item.kind else {
            return None;
        };
        let mut pending: Vec<TypeExprId> = match body {
            TypeBody::Record(fields) => fields.iter().map(|f| f.ty).collect(),
            TypeBody::Variants(variants) => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|(_, ty)| *ty))
                .collect(),
            TypeBody::Alias(ty) => vec![*ty],
            TypeBody::Distinct { field, .. } => vec![field.ty],
            TypeBody::Opaque => vec![],
        };
        pending.reverse();
        let context = FileContext {
            package: definition.0.package.clone(),
            modules: definition.0.modules.clone(),
        };
        let mut references = vec![];
        while let Some(id) = pending.pop() {
            let mut children = vec![];
            match &snapshot.tree.types[id.0 as usize].kind {
                TypeExprKind::Named { path, arguments } => {
                    children.extend(arguments.iter().filter_map(|a| match a {
                        TypeArgument::Type(t) => Some(*t),
                        TypeArgument::SizeLiteral(_) => None,
                    }));
                    let parameter = path.package.is_none()
                        && path.segments.len() == 1
                        && generics.iter().any(|g| g.name == path.segments[0]);
                    if !parameter
                        && let Ok(target) = self.lookup(&context, path).result
                        && target.0.package == definition.0.package
                        && self.engine.materialize(&target).is_ok()
                        && self.engine.items.get(&target).is_some_and(|items| {
                            items
                                .first()
                                .is_some_and(|s| matches!(s.item.kind, ItemKind::Type { .. }))
                        })
                    {
                        references.push(target);
                    }
                }
                TypeExprKind::Tuple(ts) => children.extend(ts),
                TypeExprKind::Function(f) => {
                    children.extend(f.parameters.iter().map(|p| p.ty));
                    children.extend(f.result);
                }
                TypeExprKind::Box(t) => children.push(*t),
                TypeExprKind::Qualified { base, .. } => children.push(*base),
                TypeExprKind::Error => {}
            }
            pending.extend(children.into_iter().rev());
        }
        Some(references)
    }
    /// The other members of `definition`'s group at `stage`, normalized
    /// through this recorder so their reads are this query's, and the members'
    /// texts recorded so editing any member invalidates this query.
    pub(super) fn group_at(
        &mut self,
        definition: &DefPath,
        stage: GroupStage,
    ) -> Result<RecursiveGroup, QueryError> {
        let Some(group) = self.type_group(definition) else {
            return Ok(RecursiveGroup::default());
        };
        for member in &group.members {
            if let Some(text) = self.engine.implementation_hash(member) {
                self.observed.representations.insert(member.clone(), text);
            }
        }
        let adapter = self
            .engine
            .adapters
            .signature
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let mut members = vec![];
        for member in group.members.iter().filter(|m| *m != definition) {
            let candidates = self.engine.definition_items(member)?;
            let (types, memo) = (self.engine.types.clone(), self.engine.memo.clone());
            let declarations = self.engine.frozen_declarations();
            let mut products = PhaseProducts::default();
            let signature = adapter.staged_signature(
                &types,
                &memo,
                &candidates,
                &declarations,
                &mut products,
                self,
                stage,
            )?;
            members.push(Arc::new(signature));
        }
        Ok(RecursiveGroup {
            key: group.keys.get(definition).copied(),
            members,
        })
    }
    /// Whether `definition` belongs to the owner's recursive type group: its
    /// published signature may be the one under construction, and reading it
    /// is covered by the group key rather than by a signature dependency.
    pub(super) fn in_owner_group(&mut self, definition: &DefPath) -> bool {
        let owner = self.owner.clone();
        self.type_group(&owner)
            .is_some_and(|g| g.members.contains(definition))
    }
}
