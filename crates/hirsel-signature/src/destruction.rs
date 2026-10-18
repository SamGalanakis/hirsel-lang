//! Declared cleanup effects, independent of invocation effects (MEM-5, GEN-1).
use super::*;

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
                }
                SignatureKind::Const { ty: t, .. } => map(t),
            }
            rows
        });
    }
}
