//! Declared cleanup effects, independent of invocation effects (MEM-5, GEN-1).
use super::*;
use hirsel_type_analysis::cleanup::{
    CleanupLeaf, cleanup_summary, declared_cleanup, parameter_cleanup, specialized_cleanup,
};
type HashSet<T> = IdSet<T>;

impl Normalizer<'_, '_> {
    pub(super) fn drop_summary(&self, id: TypeId) -> DropAllocation {
        self.drop_visit(id, &mut HashSet::default())
    }

    fn drop_visit(&self, id: TypeId, active: &mut HashSet<TypeId>) -> DropAllocation {
        // Nominal recursive ownership contributes its leaves. A cycle itself
        // performs only MEM-3 release; resource promises are checked on entry.
        if !active.insert(id) {
            return DropAllocation::NoAlloc;
        }
        let result = self.drop_kind(&self.types.row(id).kind, active);
        active.remove(&id);
        result
    }

    pub(super) fn drop_kind(
        &self,
        kind: &TypeKind,
        active: &mut HashSet<TypeId>,
    ) -> DropAllocation {
        cleanup_summary(kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => self.drop_visit(t, active),
            CleanupLeaf::Parameter(p) => parameter_cleanup(&self.declarations(), p),
            CleanupLeaf::Nominal(definition, arguments) => {
                let own = &self.input.syntax.item;
                if definition.path() == &own.definition {
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
                if let Some(effect) = self.group_drop.get(&definition) {
                    return specialized_cleanup(effect, arguments, |t| self.drop_visit(t, active));
                }
                let Some(Signature {
                    kind: SignatureKind::Type(def),
                    ..
                }) = self.snapshots.get(&definition).map(|s| s.as_ref())
                else {
                    return DropAllocation::MayAllocate;
                };
                declared_cleanup(def, self.types, arguments, |t| self.drop_visit(t, active))
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
        let own = DefId::intern(&self.own_definition());
        let mut effects: BTreeMap<DefId, DropAllocation> = self
            .group
            .keys()
            .chain([&own])
            .map(|d| (*d, DropAllocation::NoAlloc))
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
                                    &GenericDeclarations::signature(&d.generics),
                                    root,
                                    &mut HashSet::default(),
                                )
                            })
                    }
                    _ => DropAllocation::MayAllocate,
                };
                next.insert(*definition, effect);
            }
            // Members reach this declaration through its entry; it reaches
            // itself directly (`drop_kind`).
            next.insert(own, self.drop_summary(own_root));
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
        generics: &GenericDeclarations,
        id: TypeId,
        active: &mut HashSet<TypeId>,
    ) -> DropAllocation {
        if !active.insert(id) {
            return DropAllocation::NoAlloc;
        }
        let Some(row) = self.types.get(id) else {
            return DropAllocation::MayAllocate;
        };
        let result = cleanup_summary(&row.kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => self.member_drop(generics, t, active),
            CleanupLeaf::Parameter(p) => parameter_cleanup(generics, p),
            CleanupLeaf::Nominal(definition, arguments) => {
                if let Some(effect) = self.group_drop.get(&definition) {
                    return specialized_cleanup(effect, arguments, |t| {
                        self.member_drop(generics, t, active)
                    });
                }
                let Some(Signature {
                    kind: SignatureKind::Type(def),
                    ..
                }) = self.snapshots.get(&definition).map(|s| s.as_ref())
                else {
                    return DropAllocation::MayAllocate;
                };
                declared_cleanup(def, self.types, arguments, |t| {
                    self.member_drop(generics, t, active)
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
        let mut roots: Vec<TypeId> = self.own_representation.into_iter().collect();
        match &*kind {
            SignatureKind::Function(f) => {
                roots.extend(f.parameters.iter().map(|p| p.ty));
                roots.push(f.result);
                roots.extend(f.error);
            }
            SignatureKind::Type(t) => {
                roots.extend(t.representation);
                roots.extend(t.serializable_check.as_ref().map(|c| c.representation));
            }
            SignatureKind::Const { ty, .. } => roots.push(*ty),
        }
        let map = hirsel_check_protocol::rows::rebuild(self.types, roots, |id, row| Type {
            drop_allocation: self.drop_summary(id),
            ..row.clone()
        });
        let remap = |id: &mut TypeId| {
            if let Some(mapped) = map.get(id) {
                *id = *mapped;
            }
        };
        if let Some(root) = &mut self.own_representation {
            remap(root);
        }
        match kind {
            SignatureKind::Function(f) => {
                for p in f.parameters.iter_mut() {
                    remap(&mut p.ty);
                }
                remap(&mut f.result);
                if let Some(t) = &mut f.error {
                    remap(t);
                }
            }
            SignatureKind::Type(t) => {
                if let Some(r) = &mut t.representation {
                    remap(r);
                }
                if let Some(check) = &mut t.serializable_check {
                    remap(&mut check.representation);
                }
            }
            SignatureKind::Const { ty, .. } => remap(ty),
        }
    }
}
