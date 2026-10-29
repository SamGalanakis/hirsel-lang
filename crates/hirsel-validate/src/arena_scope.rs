//! A contextual consumable arena entry is qualified by all executable uses.
use super::*;
use hirsel_types::{ParamId, PassingMode, SignatureKind};

impl Checker<'_, '_> {
    pub(super) fn arena_scope_entry(&mut self) {
        let Some(claim) = self.body.arena_scope_grant.clone() else {
            return;
        };
        let fail = ViolationKind::TypeMismatch {
            entity: "arena scope grant entry",
        };
        let Some(slot) = hirsel_types::core_roles::arena_scope_callback(&claim.intrinsic) else {
            self.error(fail);
            return;
        };
        let signature = self.signatures.get(&claim.intrinsic);
        let reviewed = signature.is_some_and(|signature| {
            signature.definition.path() == &claim.intrinsic
                && match &signature.kind {
                    SignatureKind::Function(function) => {
                        function.parameters.get(slot as usize).is_some_and(|p| {
                            p.passing == PassingMode::Borrow
                                && match &self.t.row(p.ty).kind {
                                    TypeKind::Function(callback) => {
                                        callback.parameters.len() == 1
                                            && callback.parameters[0].passing == PassingMode::Borrow
                                            && self.arena_type(callback.parameters[0].ty)
                                    }
                                    _ => false,
                                }
                        })
                    }
                    _ => false,
                }
        });
        if !reviewed
            || self.site.body == self.definition.entry
            || claim.callback_parameter != ParamId(slot)
            || claim.arena_parameter != ParamId(0)
            || self.body.parameters.len() != 1
            || !self.arena_type(self.body.locals[self.body.parameters[0].0 as usize].ty)
        {
            self.error(fail);
            return;
        }
        let expected = self.site.body;
        let mut qualified = false;
        for (parent_index, parent) in self.definition.bodies.iter().enumerate() {
            let mut carriers = BTreeSet::new();
            loop {
                let before = carriers.len();
                for instruction in parent.blocks.iter().flat_map(|b| &b.instructions) {
                    if let InstructionKind::Assign { destination, value } = instruction.kind {
                        let produces = match &parent.rvalues[value.0 as usize] {
                            Rvalue::Closure { body, .. } => *body == expected,
                            Rvalue::Use(value) => {
                                carriers.contains(&parent.places[value.place.0 as usize].root.0)
                            }
                            _ => false,
                        };
                        if produces {
                            carriers.insert(parent.places[destination.0 as usize].root.0);
                        }
                    }
                    if let InstructionKind::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } = &instruction.kind
                    {
                        let old = parent.places[destination.0 as usize].root.0;
                        let new = parent.places[value.place.0 as usize].root.0;
                        if carriers.contains(&old) {
                            carriers.insert(parent.places[displaced.0 as usize].root.0);
                        }
                        if carriers.contains(&new) {
                            carriers.insert(old);
                        }
                    }
                }
                if carriers.len() == before {
                    break;
                }
            }
            let held =
                |value: &Operand| carriers.contains(&parent.places[value.place.0 as usize].root.0);
            for instruction in parent.blocks.iter().flat_map(|b| &b.instructions) {
                if let InstructionKind::Replace {
                    destination, value, ..
                } = &instruction.kind
                {
                    if held(value)
                        || carriers.contains(&parent.places[destination.0 as usize].root.0)
                    {
                        // A contextual entry may not be installed into mutable
                        // or erased storage and recovered under another entry.
                        self.error(fail.clone());
                    }
                }
                if let InstructionKind::Assign { value, .. } = instruction.kind {
                    let value = &parent.rvalues[value.0 as usize];
                    match value {
                        Rvalue::Use(operand) if held(operand) => {
                            if !parent.places[operand.place.0 as usize]
                                .projection
                                .is_empty()
                                || operand.usage == UseKind::Copy
                            {
                                self.error(fail.clone());
                            }
                        }
                        value
                            if super::typing::operands(value)
                                .iter()
                                .any(|operand| held(operand)) =>
                        {
                            self.error(fail.clone())
                        }
                        _ => (),
                    }
                }
            }
            for block in &parent.blocks {
                match &block.terminator {
                    Terminator::Call {
                        callee, arguments, ..
                    } => {
                        if matches!(callee, Callee::Indirect { value, .. } if held(value)) {
                            self.error(fail.clone());
                        }
                        for argument in arguments.iter().filter(|argument| held(&argument.value)) {
                            let exact = matches!(callee, Callee::Definition { path, .. } if *path == claim.intrinsic)
                                && argument.parameter == claim.callback_parameter
                                && self
                                    .local_callable_bodies(
                                        BodyId(parent_index as u32),
                                        argument.value.place,
                                    )
                                    .is_some_and(|bodies| {
                                        bodies.iter().all(|body| *body == expected)
                                    });
                            if exact {
                                qualified = true;
                            } else {
                                self.error(fail.clone());
                            }
                        }
                    }
                    Terminator::Return { value, .. } if held(value) => self.error(fail.clone()),
                    _ => (),
                }
            }
        }
        if !qualified {
            self.error(fail);
        }
    }
}
