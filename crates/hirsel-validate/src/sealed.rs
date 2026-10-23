//! MEM-4 proofs reconstructed from declarations and executable owned payloads.
use super::*;
use hirsel_types::{Bound, BuiltinType, Multiplicity, SignatureKind};

impl Checker<'_> {
    pub(super) fn seal_type(&mut self, arguments: &[CallArgument]) {
        let argument = |slot| arguments.iter().find(|a| a.parameter.0 == slot);
        let (Some(grant), Some(root)) = (argument(0), argument(1)) else {
            self.error(ViolationKind::TypeMismatch {
                entity: "seal arguments",
            });
            return;
        };
        let root_owned = root.value.usage == UseKind::Take
            || root.value.usage == UseKind::Copy
                && self.bound(
                    self.t,
                    self.place_type(root.value.place),
                    Bound::Plain,
                    self.generic_declarations(),
                );
        if grant.value.usage != UseKind::Take
            || !root_owned
            || !self.arena_type(self.place_type(grant.value.place))
            || self
                .t
                .row(self.place_type(grant.value.place))
                .allocation_target
                .is_some()
        {
            self.error(ViolationKind::InvalidUse(grant.value.place));
        }
        if !self.sealable_payload(self.place_type(root.value.place)) {
            self.error(ViolationKind::TypeMismatch {
                entity: "seal resource-free payload",
            });
        }
    }

    /// Callable headers are not owned graph edges. Their actual environments
    /// are, independently of a producer's erased resource/mode summary.
    fn sealable_payload(&mut self, root: TypeId) -> bool {
        let mut pending = vec![(root, false)];
        let mut seen = HashSet::default();
        let mut expansions = hirsel_types::NominalExpansions::default();
        while let Some((ty, exit)) = pending.pop() {
            if exit {
                expansions.leave(ty, 0);
                continue;
            }
            if !seen.insert(ty) {
                continue;
            }
            let row = self.t.row(ty).clone();
            match row.kind {
                TypeKind::Error => return false,
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    let Some(signature) = self.signatures.get(definition) else {
                        return false;
                    };
                    let SignatureKind::Type(declaration) = &signature.kind else {
                        return false;
                    };
                    if declaration.resource || declaration.capability.is_some() {
                        return false;
                    }
                    if declaration.bounds.iter().any(|b| {
                        matches!(
                            b,
                            Bound::Plain | Bound::Copyable | Bound::Data | Bound::Flat
                        )
                    }) {
                        continue;
                    }
                    if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                        return false;
                    }
                    let Some(representation) = declaration.representation else {
                        return false;
                    };
                    if !expansions
                        .enter(ty, 0, self.t)
                        .unwrap_or_else(|error| self.proof_failure(error, false))
                    {
                        continue;
                    }
                    pending.push((ty, true));
                    let expanded = hirsel_types::substitute(
                        self.t,
                        self.memo,
                        representation,
                        &hirsel_types::Substitution {
                            binder: Some(hirsel_types::GenericBinder::Bound(0)),
                            roles: &signature.metadata.roles,
                            arguments: &arguments,
                            authorities: &vec![
                                hirsel_types::AuthoritySummary::Unknown;
                                arguments.len()
                            ],
                        },
                    );
                    let Ok(expanded) = expanded else {
                        return false;
                    };
                    pending.push((expanded, false));
                }
                TypeKind::Parameter(_) => {
                    if !self.bound(self.t, ty, Bound::Copyable, self.generic_declarations()) {
                        return false;
                    }
                }
                TypeKind::Function(_) => {
                    let mut found = false;
                    for body in &self.definition.bodies {
                        for instruction in body.blocks.iter().flat_map(|b| &b.instructions) {
                            let InstructionKind::Assign { destination, value } = instruction.kind
                            else {
                                continue;
                            };
                            if !self
                                .t
                                .semantic_eq(body.places[destination.0 as usize].ty, ty)
                            {
                                continue;
                            }
                            match &body.rvalues[value.0 as usize] {
                                Rvalue::Closure { captures, .. } => {
                                    found = true;
                                    pending.extend(
                                        captures
                                            .iter()
                                            .map(|c| (body.places[c.place.0 as usize].ty, false)),
                                    );
                                }
                                Rvalue::FunctionReference { .. } => found = true,
                                _ => (),
                            }
                        }
                    }
                    if !found && row.modes.multiplicity == Multiplicity::Once {
                        return false;
                    }
                }
                TypeKind::Builtin {
                    kind:
                        BuiltinType::Shared
                        | BuiltinType::SlotId
                        | BuiltinType::Ptr
                        | BuiltinType::FnPtr,
                    ..
                } => (),
                TypeKind::Builtin {
                    kind:
                        BuiltinType::Task
                        | BuiltinType::Sender
                        | BuiltinType::Receiver
                        | BuiltinType::Region
                        | BuiltinType::Iter,
                    ..
                } => return false,
                kind => pending.extend(kind.value_parts().into_iter().map(|t| (t, false))),
            }
        }
        true
    }
}
