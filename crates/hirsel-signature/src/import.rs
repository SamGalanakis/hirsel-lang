use super::*;
impl Normalizer<'_, '_> {
    /// Shared graph transport handles binders; this wrapper owns signature reads
    /// and translates malformed imported graphs to source diagnostics.
    pub(super) fn import(
        &mut self,
        id: TypeId,
        source: &TypeStore,
        args: &[GenericArgument],
        _map: &mut BTreeMap<TypeId, TypeId>,
        _active: &mut BTreeSet<TypeId>,
    ) -> Result<TypeId, QueryError> {
        let authorities: Vec<_> = args
            .iter()
            .map(|arg| match arg {
                GenericArgument::Type(t) => self.type_authority(Some(*t)),
                GenericArgument::Size(_) => AuthoritySummary::inert(),
            })
            .collect();
        let start = self.types.len();
        let result = substitute_type(
            source,
            id,
            &mut self.types,
            Substitution {
                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                roles: &NominalRoles {
                    unsafe_capability: self.input.metadata.unsafe_capability.clone(),
                    result_type: self.input.metadata.result_type.clone(),
                    tasks_capability: self.input.metadata.tasks_capability.clone(),
                    scope_capability: self.input.metadata.scope_capability.clone(),
                    heap: self.input.metadata.heap.clone(),
                },
                arguments: args,
                authorities: &authorities,
            },
        )
        .map_err(|error| {
            diagnostic(
                if matches!(error, GraphError::StructuralCycle(_)) {
                    const { RuleId::of("LANG-7") }
                } else {
                    const { RuleId::of("LANG-6") }
                },
                self.span,
                "invalid imported signature type graph or substitution",
            )
        })?;
        let owner = &self.input.tree.items[self.input.item.0 as usize].definition;
        let dependencies: BTreeSet<_> = self
            .types
            .iter()
            .skip(start)
            .filter_map(|(_, t)| match &t.kind {
                TypeKind::Named { definition, .. } if definition != owner => {
                    Some(definition.clone())
                }
                _ => None,
            })
            .collect();
        for definition in dependencies {
            self.snapshot(&definition)?;
        }
        // Transport preserves occurrence qualifiers. A substituted nominal
        // Plain argument needs its declaration to erase inert locality and
        // uniqueness before this imported callback header is published.
        let generics = self.declarations();
        let mut types = std::mem::take(&mut self.types);
        let normalized = canonical_plain_type_in(&mut types, result, &generics, &mut |path| {
            self.property_signature(path)
        });
        self.types = types;
        normalized
    }
}
