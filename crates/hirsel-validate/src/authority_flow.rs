//! Capability authority follows values derived inside the frame (spec #58).
use super::*;

impl Checker<'_> {
    pub(super) fn initial_authority(&self, model: &Model, state: &mut State) {
        let allowed = self.frame_authority();
        for (root, local) in self.body.locals.iter().enumerate() {
            let valid = self.body.parameters.contains(&LocalId(root as u32))
                || local.storage == Storage::Capture
                    && allowed.as_ref().is_none_or(|allowed| {
                        self.authority_with_generics(self.t, local.ty, &self.generic_declarations())
                            .is_subset_of(allowed)
                    });
            for &part in &model.root_parts[root] {
                state.must[part].set(MustFacts::AUTHORITY, valid);
            }
        }
    }

    pub(super) fn operand_authority(&self, model: &Model, state: &State, place: PlaceId) -> bool {
        model.inert[model.mapping[place.0 as usize]]
            || model
                .parts(place)
                .all(|part| state.must[part].has(MustFacts::AUTHORITY))
    }

    pub(super) fn set_authority(
        &self,
        model: &Model,
        state: &mut State,
        place: PlaceId,
        valid: bool,
    ) {
        for part in model.parts(place) {
            state.must[part].set(MustFacts::AUTHORITY, valid);
        }
    }

    pub(super) fn rvalue_authority(&self, model: &Model, state: &State, value: &Rvalue) -> bool {
        let inputs = typing::operands(value)
            .into_iter()
            .all(|value| self.operand_authority(model, state, value.place));
        let ambient = match value {
            Rvalue::FunctionReference { definition, arguments } => self.signatures.get(definition).and_then(|signature| match &signature.kind {
                SignatureKind::Function(function) => {
                    let generics: Vec<_> = if arguments.is_empty() {
                        function.generics.iter().enumerate().map(|(ordinal, parameter)| {
                            if matches!(parameter, hirsel_types::GenericParameter::Size { .. }) || matches!(parameter, hirsel_types::GenericParameter::Type { bounds, .. } if hirsel_types::bounds_imply_inert(bounds)) {
                                hirsel_types::AuthoritySummary::inert()
                            } else {
                                hirsel_types::AuthoritySummary::Known(vec![hirsel_types::AuthorityTerm::TypeParameter(hirsel_types::TypeParameterRef::bound(hirsel_types::TypeParamId(ordinal as u32)))].into())
                            }
                        }).collect()
                    } else {
                        arguments.iter().map(|argument| match argument {
                            hirsel_types::GenericArgument::Type(ty) => self.authority_with_generics(self.t, *ty, &self.generic_declarations()),
                            _ => hirsel_types::AuthoritySummary::inert(),
                        }).collect()
                    };
                    Some(Self::instantiated_authority(&function.authority, &vec![hirsel_types::AuthoritySummary::inert(); function.parameters.len()], &generics))
                }
                _ => None,
            }),
            _ => None,
        };
        inputs
            && ambient.is_none_or(|ambient| {
                self.frame_authority()
                    .is_none_or(|allowed| ambient.is_subset_of(&allowed))
            })
    }

    pub(super) fn call_authority(
        &mut self,
        model: &Model,
        state: &State,
        callee: &Callee,
        arguments: &[CallArgument],
        report: bool,
    ) -> bool {
        let valid = arguments
            .iter()
            .all(|argument| self.operand_authority(model, state, argument.value.place))
            && match callee {
                Callee::Indirect { value, .. } => self.operand_authority(model, state, value.place),
                Callee::Definition { .. } => true,
            };
        if self.frame_authority().is_some() && !valid {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "body uses",
                },
            );
        }
        valid
    }
}
