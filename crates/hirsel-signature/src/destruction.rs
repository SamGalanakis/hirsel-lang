//! Declared cleanup effects, independent of invocation effects (MEM-5, GEN-1).
use super::*;
use hirsel_type_analysis::cleanup::{
    CleanupLeaf, cleanup_summary, declared_cleanup, parameter_cleanup, specialized_cleanup,
};

impl Normalizer<'_, '_> {
    pub(super) fn drop_summary(&self, id: TypeId) -> DropAllocation {
        self.drop_visit(id, &mut BTreeSet::new())
    }

    fn drop_visit(&self, id: TypeId, active: &mut BTreeSet<TypeId>) -> DropAllocation {
        // Nominal recursive ownership contributes its leaves. A cycle itself
        // performs only MEM-3 release; resource promises are checked on entry.
        if !active.insert(id) {
            return DropAllocation::NoAlloc;
        }
        let result = self.drop_kind(&self.types.get(id).unwrap().kind, active);
        active.remove(&id);
        result
    }

    pub(super) fn drop_kind(
        &self,
        kind: &TypeKind,
        active: &mut BTreeSet<TypeId>,
    ) -> DropAllocation {
        cleanup_summary(kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => self.drop_visit(t, active),
            CleanupLeaf::Parameter(p) => parameter_cleanup(&self.declarations(), p),
            CleanupLeaf::Nominal(definition, arguments) => {
                let own = &self.input.tree.items[self.input.item.0 as usize];
                if *definition == own.definition {
                    if let ast::ItemKind::Type { flavor, drop, .. } = &own.kind {
                        if own.visibility == ast::Visibility::PublicOpaque {
                            if let ast::ItemKind::Type {
                                bounds: Some(bs), ..
                            } = &own.kind
                            {
                                if bs.contains(&Bound::NoallocDrop) || bs.contains(&Bound::Plain) {
                                    return DropAllocation::NoAlloc;
                                }
                            }
                            return DropAllocation::MayAllocate;
                        }
                        if *flavor == ast::TypeFlavor::Resource
                            && !drop.as_ref().is_some_and(|d| d.noalloc)
                        {
                            return DropAllocation::MayAllocate;
                        }
                        return self
                            .own_representation
                            .map_or(DropAllocation::NoAlloc, |r| self.drop_visit(r, active));
                    }
                }
                if let Some(effect) = self.group_drop.get(definition) {
                    return specialized_cleanup(effect, arguments, |t| self.drop_visit(t, active));
                }
                let Some(Signature {
                    kind: SignatureKind::Type(def),
                    types,
                    ..
                }) = self.snapshots.get(definition).map(|s| s.as_ref())
                else {
                    return DropAllocation::MayAllocate;
                };
                declared_cleanup(def, types, arguments, |t| self.drop_visit(t, active))
            }
        })
    }

    /// A recursive group's cleanup effects are one fixed point over its
    /// members' representations, as a lone recursive declaration's is over
    /// its own (`drop_visit`): every member starts nonallocating over its own
    /// parameters and joins what its fields reach, until no member's effect
    /// grows. A cycle itself performs only MEM-3 release. Provisional members
    /// carry no finalizer promise or declared bounds, so a resource, opaque or
    /// capability member stays allocating.
    pub(super) fn settle_group_drop(&mut self) {
        if self.group.is_empty() {
            return;
        }
        let Some(own_root) = self.own_representation else {
            return;
        };
        let own = self.own_definition();
        let mut effects: BTreeMap<ast::DefPath, DropAllocation> = self
            .group
            .keys()
            .chain([&own])
            .map(|d| (d.clone(), DropAllocation::NoAlloc))
            .collect();
        loop {
            self.group_drop = effects.clone();
            let mut next = BTreeMap::new();
            for (definition, member) in &self.group {
                let effect = match &member.kind {
                    SignatureKind::Type(d)
                        if !d.resource
                            && d.capability.is_none()
                            && member.metadata.visibility != ast::Visibility::PublicOpaque =>
                    {
                        d.representation
                            .map_or(DropAllocation::MayAllocate, |root| {
                                self.member_drop(
                                    &member.types,
                                    &GenericDeclarations::signature(&d.generics),
                                    root,
                                    &mut BTreeSet::new(),
                                )
                            })
                    }
                    _ => DropAllocation::MayAllocate,
                };
                next.insert(definition.clone(), effect);
            }
            // Members reach this declaration through its entry; it reaches
            // itself directly (`drop_kind`).
            next.insert(own.clone(), self.drop_summary(own_root));
            if next == effects {
                self.group_drop.remove(&own);
                return;
            }
            effects = next;
        }
    }

    /// `drop_visit` over another member's provisional graph, whose type
    /// parameters are that member's own.
    fn member_drop(
        &self,
        types: &TypeStore,
        generics: &GenericDeclarations,
        id: TypeId,
        active: &mut BTreeSet<TypeId>,
    ) -> DropAllocation {
        if !active.insert(id) {
            return DropAllocation::NoAlloc;
        }
        let Some(row) = types.get(id) else {
            return DropAllocation::MayAllocate;
        };
        let result = cleanup_summary(&row.kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => self.member_drop(types, generics, t, active),
            CleanupLeaf::Parameter(p) => parameter_cleanup(generics, p),
            CleanupLeaf::Nominal(definition, arguments) => {
                if let Some(effect) = self.group_drop.get(definition) {
                    return specialized_cleanup(effect, arguments, |t| {
                        self.member_drop(types, generics, t, active)
                    });
                }
                let Some(Signature {
                    kind: SignatureKind::Type(def),
                    types: declared,
                    ..
                }) = self.snapshots.get(definition).map(|s| s.as_ref())
                else {
                    return DropAllocation::MayAllocate;
                };
                declared_cleanup(def, declared, arguments, |t| {
                    self.member_drop(types, generics, t, active)
                })
            }
        });
        active.remove(&id);
        result
    }

    /// Publish a fresh immutable graph with complete summaries, including nominal
    /// recursive references whose representation was unavailable during traversal.
    pub(super) fn finish_drop_summaries(&mut self, kind: &mut SignatureKind) {
        self.generics = match kind {
            SignatureKind::Function(f) => f.generics.to_vec(),
            SignatureKind::Type(t) => t.generics.clone(),
            SignatureKind::Const { ty: _, .. } => vec![],
        };
        let rows: Vec<_> = self
            .types
            .iter()
            .map(|(id, t)| Type {
                drop_allocation: self.drop_summary(id),
                ..t.clone()
            })
            .collect();
        self.types = TypeStore::from_graph(rows.len(), |ids| {
            let map = |id: &mut TypeId| *id = ids[id.index()];
            let function = |f: &mut FunctionType| {
                for p in &mut f.parameters {
                    map(&mut p.ty);
                }
                map(&mut f.result);
                if let Some(t) = &mut f.error {
                    map(t);
                }
            };
            let mut rows = rows;
            for t in &mut rows {
                match &mut t.kind {
                    TypeKind::Tuple(ts) => ts.iter_mut().for_each(map),
                    TypeKind::Record(fs) => fs.iter_mut().for_each(|f| map(&mut f.ty)),
                    TypeKind::Variant(vs) => vs
                        .iter_mut()
                        .for_each(|v| v.fields.iter_mut().for_each(|(_, t)| map(t))),
                    TypeKind::Box(t) => map(t),
                    TypeKind::Function(f) => function(f),
                    TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                        for a in arguments {
                            if let GenericArgument::Type(t) = a {
                                map(t);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Some(root) = &mut self.own_representation {
                map(root);
            }
            match kind {
                SignatureKind::Function(f) => function(f),
                SignatureKind::Type(t) => {
                    if let Some(r) = &mut t.representation {
                        map(r);
                    }
                    if let Some(check) = &mut t.serializable_check {
                        map(&mut check.representation);
                    }
                }
                SignatureKind::Const { ty: t, .. } => map(t),
            }
            rows
        });
    }
}
