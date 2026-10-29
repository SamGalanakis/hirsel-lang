//! MEM-4 proofs reconstructed from declarations and executable owned payloads.
use super::*;
use hirsel_types::{Bound, BuiltinType, Multiplicity, SignatureKind};

impl Checker<'_, '_> {
    pub(super) fn seal_type(&mut self, arguments: &[CallArgument]) {
        if self
            .invocation_header()
            .is_some_and(|header| header.allocation == hirsel_types::Allocation::NoAlloc)
            || self.noalloc_body()
        {
            self.error(ViolationKind::TypeMismatch {
                entity: "seal allocation",
            });
        }
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
        if !self.sealable_payload(root.value.place) {
            self.error(ViolationKind::TypeMismatch {
                entity: "seal resource-free payload",
            });
        }
    }

    /// Callable headers are not owned graph edges. Their actual environments
    /// are, independently of a producer's erased resource/mode summary.
    fn sealable_payload(&mut self, root: PlaceId) -> bool {
        let mut pending = vec![(self.place_type(root), false)];
        let mut environments = false;
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
                    if row.modes.multiplicity == Multiplicity::Once && !environments {
                        let Some(captures) = self.seal_owned_captures(root) else {
                            return false;
                        };
                        pending.extend(captures.into_iter().map(|ty| (ty, false)));
                        environments = true;
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

    /// Follow this operand's producers, including aggregate fields and capture
    /// sites. A different closure of the same type proves nothing about it.
    fn seal_owned_captures(&mut self, root: PlaceId) -> Option<Vec<TypeId>> {
        let mut pending = vec![(self.site.body, root, Vec::<Projection>::new())];
        let mut seen = HashSet::default();
        let mut captures = vec![];
        while let Some((body_id, place, suffix)) = pending.pop() {
            let body = &self.definition.bodies[body_id.0 as usize];
            let access = &body.places[place.0 as usize];
            if self.bound(
                self.t,
                access.ty,
                Bound::Copyable,
                self.generic_declarations(),
            ) {
                continue;
            }
            let mut projection = access.projection.to_vec();
            projection.extend(suffix);
            if !seen.insert((body_id.0, access.root.0, projection.clone())) {
                continue;
            }
            if body.locals[access.root.0 as usize].storage == Storage::Capture {
                let ordinal = body
                    .locals
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.storage == Storage::Capture)
                    .position(|(index, _)| index == access.root.0 as usize)?;
                let mut found = false;
                for (parent_index, parent) in self.definition.bodies.iter().enumerate() {
                    for rvalue in &parent.rvalues {
                        if let Rvalue::Closure {
                            body: child,
                            captures: inputs,
                            ..
                        } = rvalue
                        {
                            if *child == body_id {
                                let input = inputs.get(ordinal)?;
                                if input.usage == UseKind::Mut {
                                    return None;
                                }
                                pending.push((
                                    BodyId(parent_index as u32),
                                    input.place,
                                    projection.clone(),
                                ));
                                found = true;
                            }
                        }
                    }
                }
                if !found {
                    return None;
                }
                continue;
            }
            if body.locals[access.root.0 as usize].storage == Storage::Parameter {
                return None;
            }
            let mut found = false;
            for block in &body.blocks {
                if let Terminator::Call {
                    returns, arguments, ..
                } = &block.terminator
                {
                    if returns
                        .as_ref()
                        .is_some_and(|r| body.places[r.destination.0 as usize].root == access.root)
                        || arguments.iter().any(|a| {
                            a.value.usage == UseKind::Mut
                                && body.places[a.value.place.0 as usize].root == access.root
                        })
                    {
                        return None;
                    }
                }
                for instruction in &block.instructions {
                    if matches!(&instruction.kind, InstructionKind::Replace { destination, .. } | InstructionKind::BeginLoan { place: destination, mutable: true, .. } if body.places[destination.0 as usize].root == access.root)
                    {
                        return None;
                    }
                    let InstructionKind::Assign { destination, value } = instruction.kind else {
                        continue;
                    };
                    let destination = &body.places[destination.0 as usize];
                    if destination.root != access.root {
                        continue;
                    }
                    if !destination.projection.is_empty() {
                        return None;
                    }
                    found = true;
                    match &body.rvalues[value.0 as usize] {
                        Rvalue::Use(input)
                        | Rvalue::Unary {
                            op: hirsel_syntax::UnaryOp::Unique,
                            value: input,
                        } => pending.push((body_id, input.place, projection.clone())),
                        Rvalue::Construct { fields, .. } => {
                            if let Some((first, rest)) = projection.split_first() {
                                let field = match first {
                                    Projection::Field(field)
                                    | Projection::VariantField { field, .. } => Some(*field),
                                    Projection::Index(_) => None,
                                };
                                for input in fields
                                    .iter()
                                    .filter(|f| field.is_none_or(|field| f.field == field))
                                {
                                    pending.push((body_id, input.value.place, rest.to_vec()));
                                }
                            } else {
                                pending.extend(
                                    fields.iter().map(|f| (body_id, f.value.place, vec![])),
                                );
                            }
                        }
                        Rvalue::Closure {
                            captures: inputs, ..
                        } if projection.is_empty() => {
                            for input in inputs {
                                let ty = body.places[input.place.0 as usize].ty;
                                captures.push(ty);
                                pending.push((body_id, input.place, vec![]));
                            }
                        }
                        Rvalue::FunctionReference { .. } if projection.is_empty() => (),
                        _ => return None,
                    }
                }
            }
            if !found {
                return None;
            }
        }
        Some(captures)
    }
}
