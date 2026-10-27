//! Independent reconstruction of the closed CPU callback restriction from TIR.
use super::*;
use hirsel_types::{AuthoritySummary, BuiltinType, Uses};

impl Checker<'_> {
    pub(super) fn cpu_call(&mut self, arguments: &[CallArgument]) {
        for argument in arguments.iter().filter(|a| matches!(a.parameter.0, 2 | 3)) {
            let ty = self.place_type(argument.value.place);
            let permitted = matches!(&self.t.row(ty).kind, TypeKind::Function(f)
                if f.foreign.is_none() && matches!(&f.uses, Uses::Only(AuthoritySummary::Known(terms)) if terms.is_empty()));
            let values_permitted = match &self.t.row(ty).kind {
                TypeKind::Function(f) => {
                    f.parameters.iter().all(|p| self.cpu_type(p.ty)) && self.cpu_type(f.result)
                }
                _ => false,
            };
            if !permitted || !values_permitted {
                self.error(ViolationKind::TypeMismatch {
                    entity: "closed CPU callback",
                });
                continue;
            }
            let Some(bodies) = self.local_callable_bodies(self.site.body, argument.value.place)
            else {
                self.error(ViolationKind::TypeMismatch {
                    entity: "CPU callback without a local closed-call proof",
                });
                continue;
            };
            let mut pending = bodies;
            let mut seen = BTreeSet::new();
            while let Some(body_id) = pending.pop() {
                if !seen.insert(body_id.0) {
                    continue;
                }
                let body = &self.definition.bodies[body_id.0 as usize];
                for local in body.locals.iter().filter(|l| l.storage == Storage::Capture) {
                    if !self.cpu_type(local.ty) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "CPU authority capture",
                        });
                    }
                }
                for value in &body.rvalues {
                    let path = match value {
                        Rvalue::FunctionReference { definition, .. } => Some(definition),
                        Rvalue::Construct {
                            constructor: Constructor::Record(path),
                            ..
                        }
                        | Rvalue::Construct {
                            constructor:
                                Constructor::Variant {
                                    definition: path, ..
                                },
                            ..
                        } if self.signatures.get(path).is_some_and(|s| {
                            matches!(&s.kind,
                                hirsel_types::SignatureKind::Type(t) if t.resource)
                        }) =>
                        {
                            Some(path)
                        }
                        _ => None,
                    };
                    if path.is_some_and(|path| !path.0.package.package_is("core", "1")) {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "unproved CPU helper or finalizer",
                        });
                    }
                    if let Rvalue::Closure { body, .. } = value {
                        pending.push(*body);
                    }
                }
                for block in &body.blocks {
                    if let Terminator::Call {
                        callee: Callee::Definition { path, .. },
                        ..
                    } = &block.terminator
                        && !path.0.package.package_is("core", "1")
                    {
                        self.error(ViolationKind::TypeMismatch {
                            entity: "unproved external CPU helper",
                        });
                    }
                    if let Terminator::Call {
                        callee: Callee::Indirect { value, .. },
                        ..
                    } = &block.terminator
                    {
                        if let Some(targets) = self.local_callable_bodies(body_id, value.place) {
                            pending.extend(targets);
                        } else {
                            let ty = body.places[value.place.0 as usize].ty;
                            if !matches!(&self.t.row(ty).kind, TypeKind::Function(f)
                                if f.foreign.is_none() && matches!(&f.uses, Uses::Only(AuthoritySummary::Known(terms)) if terms.is_empty()))
                            {
                                self.error(ViolationKind::TypeMismatch {
                                    entity: "unproved indirect CPU call",
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    fn cpu_type(&self, root: TypeId) -> bool {
        if !matches!(self.authority(root), AuthoritySummary::Known(terms) if terms.is_empty()) {
            return false;
        }
        let mut pending = vec![root];
        let mut seen = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            match &self.t.row(ty).kind {
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    if self.signatures.get(*definition).is_some_and(|s| matches!(&s.kind,
                        hirsel_types::SignatureKind::Type(t) if t.resource || t.capability.is_some())) { return false; }
                    if let Some(s) = self.signatures.get(*definition)
                        && let hirsel_types::SignatureKind::Type(t) = &s.kind
                        && let Some(representation) = t.representation
                    {
                        let representation = hirsel_types::substitute(
                            self.t,
                            self.memo,
                            representation,
                            &hirsel_types::Substitution {
                                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                                roles: &s.metadata.roles,
                                arguments,
                                authorities: &vec![AuthoritySummary::Unknown; arguments.len()],
                            },
                        );
                        match representation {
                            Ok(t) => pending.push(t),
                            Err(_) => return false,
                        }
                    }
                    pending.extend(arguments.iter().filter_map(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some(*t)
                        } else {
                            None
                        }
                    }));
                }
                TypeKind::Builtin { kind, arguments } => {
                    if matches!(
                        kind,
                        BuiltinType::Task
                            | BuiltinType::Mutex
                            | BuiltinType::Atomic
                            | BuiltinType::Sender
                            | BuiltinType::Receiver
                            | BuiltinType::Ptr
                            | BuiltinType::FnPtr
                    ) {
                        return false;
                    }
                    pending.extend(arguments.iter().filter_map(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some(*t)
                        } else {
                            None
                        }
                    }));
                }
                TypeKind::Function(_) => return false,
                TypeKind::Tuple(parts) => pending.extend(parts.iter().copied()),
                TypeKind::Variant(variants) => pending.extend(
                    variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|(_, t)| *t)),
                ),
                TypeKind::Parameter(_) => return false,
                TypeKind::Box(t) => pending.push(*t),
                TypeKind::Record(fields) => pending.extend(fields.iter().map(|f| f.ty)),
                _ => (),
            }
        }
        true
    }
}
