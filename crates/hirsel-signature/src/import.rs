use super::*;
impl Normalizer<'_, '_> {
    /// Shared graph transport handles binders; this wrapper owns signature reads
    /// and translates malformed imported graphs to source diagnostics.
    pub(super) fn import(
        &mut self,
        id: TypeId,
        args: &[GenericArgument],
    ) -> Result<TypeId, QueryError> {
        let authorities: Vec<_> = args
            .iter()
            .map(|arg| match arg {
                GenericArgument::Type(t) => self.type_authority(Some(*t)),
                GenericArgument::Size(_) => AuthoritySummary::inert(),
            })
            .collect();
        let result = substitute(
            self.types,
            self.memo,
            id,
            &Substitution {
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
        let owner = &self.input.syntax.item.definition;
        let dependencies: BTreeSet<_> = closure(self.types, [result])
            .iter()
            .filter_map(|t| match &self.types.row(*t).kind {
                TypeKind::Named { definition, .. } if definition.path() != owner => {
                    Some(*definition)
                }
                _ => None,
            })
            .collect();
        for definition in dependencies {
            self.snapshot(definition)?;
        }
        // Transport preserves occurrence qualifiers. A substituted nominal
        // Plain argument needs its declaration to erase inert locality and
        // uniqueness before this imported callback header is published.
        let generics = self.declarations();
        let types = self.types;
        canonical_plain_type_in(types, result, &generics, &mut |definition| {
            self.property_signature(definition)
        })
    }
}
