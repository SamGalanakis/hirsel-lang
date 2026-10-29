//! Capability authority follows values derived inside the frame (spec #58).
use super::*;
use hirsel_types::{AuthoritySummary, AuthorityTerm, ParameterRef, SignatureKind, Uses};

// Unknown is missing evidence, never a finite restriction's permission.
fn within(actual: &AuthoritySummary, allowed: &AuthoritySummary) -> bool {
    actual == &AuthoritySummary::inert()
        || matches!(allowed, AuthoritySummary::Known(_)) && actual.is_subset_of(allowed)
}

impl Checker<'_, '_> {
    /// Resolve a selector against V's executable header and declaration-order
    /// body parameters. A receipt belongs only to this invocation's own slot.
    pub(crate) fn own_parameter_selected(&self, ordinal: usize) -> bool {
        ordinal < self.body.parameters.len()
            && self.invocation_header().is_some_and(|header| {
                ordinal < header.parameters.len()
                    && matches!(&header.uses, Uses::Only(AuthoritySummary::Known(terms))
                        if terms.contains(&AuthorityTerm::Parameter(ParameterRef::bound(hirsel_types::ParamId(ordinal as u32)))))
            })
    }

    pub(super) fn initial_authority(&self, model: &Model, state: &mut State) {
        let allowed = self.frame_authority();
        for (root, local) in self.body.locals.iter().enumerate() {
            let parameter = self
                .body
                .parameters
                .iter()
                .position(|p| p.0 as usize == root);
            let valid = parameter.is_some_and(|ordinal| {
                if self.site.body != self.definition.entry || allowed.is_none() {
                    return true;
                }
                let authority =
                    self.authority_with_generics(self.t, local.ty, &self.generic_declarations());
                within(&authority, allowed.as_ref().unwrap())
                    || self.own_parameter_selected(ordinal)
            }) || local.storage == Storage::Capture
                && allowed.as_ref().is_none_or(|allowed| {
                    self.authority_with_generics(self.t, local.ty, &self.generic_declarations())
                        .is_subset_of(allowed)
                });
            for &part in &model.root_parts[root] {
                state.must.set_fact(part, MustFacts::AUTHORITY, valid);
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
            state.must.set_fact(part, MustFacts::AUTHORITY, valid);
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
        let permitted = if self.site.body == self.definition.entry {
            self.entry_call_authority(model, state, callee, arguments)
        } else {
            valid
        };
        if self.frame_authority().is_some() && !permitted {
            self.flow_error(
                report,
                ViolationKind::TypeMismatch {
                    entity: "body uses",
                },
            );
        }
        valid
    }

    /// Check the named body's written uses against this call's actual receipts.
    /// Parameter IDs select declaration slots, independently of operand order.
    /// Flow discharges authorized derivations; an unselected operand retains its
    /// own summary so one selected receipt cannot hide a known sibling.
    fn entry_call_authority(
        &self,
        model: &Model,
        state: &State,
        callee: &Callee,
        arguments: &[CallArgument],
    ) -> bool {
        let Some(allowed) = self.frame_authority() else {
            return true;
        };
        let (function, generics) = match callee {
            Callee::Definition { path, arguments } => {
                let Some(signature) = self.signatures.get(path) else {
                    return false;
                };
                let SignatureKind::Function(function) = &signature.kind else {
                    return false;
                };
                (function, arguments)
            }
            Callee::Indirect {
                value, arguments, ..
            } => {
                let Some(row) = self.t.get(self.place_type(value.place)) else {
                    return false;
                };
                let TypeKind::Function(function) = &row.kind else {
                    return false;
                };
                (function, arguments)
            }
        };
        let receipt = |place| {
            if self.operand_authority(model, state, place) {
                AuthoritySummary::inert()
            } else {
                self.authority_with_generics(
                    self.t,
                    self.place_type(place),
                    &self.generic_declarations(),
                )
            }
        };
        let mut parameters = vec![AuthoritySummary::Unknown; function.parameters.len()];
        for argument in arguments {
            if let Some(parameter) = parameters.get_mut(argument.parameter.0 as usize) {
                *parameter = receipt(argument.value.place);
            }
        }
        let generics: Vec<_> = generics
            .iter()
            .map(|argument| match argument {
                GenericArgument::Type(ty) => {
                    self.authority_with_generics(self.t, *ty, &self.generic_declarations())
                }
                GenericArgument::Size(_) => AuthoritySummary::inert(),
            })
            .collect();
        let selected_callback = function.authority == AuthoritySummary::Unknown
            && matches!(callee, Callee::Indirect { value, .. }
            if self.invocation_header().is_some_and(|header| match &header.uses {
                Uses::Only(selected) => self.parameter_callback(value.place, Some(selected)),
                Uses::Unrestricted => false,
            }));
        let mut required = if selected_callback {
            AuthoritySummary::inert()
        } else {
            Self::instantiated_authority(&function.authority, &parameters, &generics)
        };
        if let Uses::Only(uses) = &function.uses {
            // An inferred closure's unknown capture ceiling carries no new
            // charge after this exact origin has been discharged. Known uses
            // and the actual callback operands remain independent obligations.
            if !selected_callback || *uses != AuthoritySummary::Unknown {
                required =
                    required.join(Self::instantiated_authority(uses, &parameters, &generics));
            }
        }
        for argument in arguments {
            if function
                .parameters
                .get(argument.parameter.0 as usize)
                .is_some_and(|p| {
                    p.passing == hirsel_syntax::PassingMode::Borrow
                        && matches!(
                            self.t.get(p.ty).map(|t| &t.kind),
                            Some(TypeKind::Function(_))
                        )
                })
            {
                required = required.join(receipt(argument.value.place));
            }
        }
        if let Callee::Indirect { value, .. } = callee {
            required = required.join(receipt(value.place));
        }
        within(&required, &allowed)
    }
}
