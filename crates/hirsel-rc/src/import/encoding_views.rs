//! Synchronous encoding can view a stable borrowed text or text-list field.
//! This is a physical proof, independent of source acceptance and take/mut modes.
use super::*;
use hirsel_types::{BuiltinType, Primitive, TypeKind};

impl Importer<'_> {
    pub(super) fn encoding_copies(&mut self) -> R<()> {
        let body = self.body;
        let n = body.locals.len();
        let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
        let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
        let aliases = self.aliases.clone();
        let used_root = |mut p: tir::PlaceId| {
            let mut seen = BTreeSet::new();
            while let Some(base) = aliases[root(p)] {
                if !seen.insert(root(p)) {
                    break;
                }
                p = base;
            }
            root(p)
        };
        let mut source = vec![None; n];
        let mut definitions = vec![0; n];
        let mut invalid = vec![false; n];
        let mut stable = vec![true; n];
        let mut used = vec![false; n];
        for block in &body.blocks {
            for inst in &block.instructions {
                use tir::InstructionKind as K;
                match &inst.kind {
                    K::Assign { destination, value } => {
                        let d = root(*destination);
                        definitions[d] += 1;
                        if let tir::Rvalue::Use(op) = &body.rvalues[value.0 as usize]
                            && op.usage == UseKind::Copy
                            && whole(*destination)
                        {
                            source[d] = Some(op.place);
                            invalid[root(op.place)] = true;
                        } else {
                            invalid[d] = true;
                            for op in rvalue_operands(&body.rvalues[value.0 as usize]) {
                                if op.usage != UseKind::Borrow {
                                    invalid[root(op.place)] = true;
                                }
                                if matches!(op.usage, UseKind::Take | UseKind::Mut) {
                                    stable[root(op.place)] = false;
                                }
                            }
                        }
                    }
                    K::BeginLoan {
                        place,
                        mutable: true,
                        ..
                    } => {
                        invalid[root(*place)] = true;
                        stable[root(*place)] = false;
                    }
                    K::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => {
                        for p in [*destination, value.place, *displaced] {
                            invalid[root(p)] = true;
                            stable[root(p)] = false;
                        }
                    }
                    K::Snapshot { destination, value } => {
                        invalid[root(*destination)] = true;
                        invalid[root(value.place)] = true;
                        stable[root(*destination)] = false;
                    }
                    K::Check { condition, .. } => invalid[root(condition.place)] = true,
                    _ => {}
                }
            }
            match &block.terminator {
                tir::Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    ..
                } => {
                    if let Some(ret) = returns {
                        invalid[root(ret.destination)] = true;
                        definitions[root(ret.destination)] += 1;
                    }
                    if let tir::Callee::Indirect { value, .. } = callee {
                        invalid[root(value.place)] = true;
                    }
                    let length = matches!(callee, tir::Callee::Definition { path, .. } if path.is_core("list.len") || path.is_core("bytes.len") || path.is_core("string.byte_len"));
                    let writer = length
                        || (matches!(callee, tir::Callee::Definition { .. })
                            && self.byte_writer(arguments, *returns)?);
                    for a in arguments {
                        let l = used_root(a.value.place);
                        used[l] = true;
                        if !writer || a.value.usage != UseKind::Borrow {
                            invalid[l] = true;
                        }
                        if matches!(a.value.usage, UseKind::Take | UseKind::Mut) {
                            stable[l] = false;
                        }
                    }
                }
                tir::Terminator::Return { value, .. }
                | tir::Terminator::Branch {
                    condition: value, ..
                }
                | tir::Terminator::Switch { value, .. } => invalid[root(value.place)] = true,
                _ => {}
            }
        }
        let scopes = super::flat_views::scope_intervals(body);
        for l in 0..n {
            let Some(source) = source[l] else { continue };
            if invalid[l]
                || !used[l]
                || definitions[l] != 1
                || body.locals[l].storage != tir::Storage::Temporary
                || self.aliases[l].is_some()
            {
                continue;
            }
            let ty = self.ty(body.locals[l].ty)?;
            let eligible = match self.types.get(ty).map(|t| &t.kind) {
                Some(TypeKind::Primitive(Primitive::String | Primitive::Bytes)) => true,
                Some(TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments,
                }) => {
                    matches!(arguments.as_ref(), [GenericArgument::Type(t)] if matches!(self.types.get(*t).map(|t| &t.kind), Some(TypeKind::Primitive(Primitive::String))))
                }
                _ => false,
            };
            if !eligible {
                continue;
            }
            let p = self.place(source)?;
            // Parameters are held throughout this invocation. An owned source
            // and computed indexes need a separate stability/lifetime proof.
            if !self.borrowed_storage(p.slot)
                || !stable[root(source)]
                || body.places[source.0 as usize]
                    .projection
                    .iter()
                    .any(|p| matches!(p, tir::Projection::Index(_)))
            {
                continue;
            }
            let outer = scopes[body.locals[root(source)].scope.0 as usize];
            let inner = scopes[body.locals[l].scope.0 as usize];
            if outer.0 <= inner.0 && inner.1 <= outer.1 {
                self.aliases[l] = Some(source);
                self.encoding_views.insert(l);
            }
        }
        Ok(())
    }

    /// No retained result and only one mutable argument, a byte list that
    /// cannot store a String/Bytes owner (including through a nested field).
    /// The call keeps its declared ABI; no taken argument becomes borrowed.
    fn byte_writer(
        &mut self,
        arguments: &[tir::CallArgument],
        returns: Option<tir::CallReturn>,
    ) -> R<bool> {
        let Some(ret) = returns else { return Ok(false) };
        let result = self.place_layout(ret.destination)?;
        if !self.layouts.record(result).drop.is_empty() {
            return Ok(false);
        }
        let mut writers = 0;
        for a in arguments {
            if a.value.usage == UseKind::Take {
                return Ok(false);
            }
            if a.value.usage != UseKind::Mut {
                let ty = self.ty(self.body.places[a.value.place.0 as usize].ty)?;
                if !matches!(
                    self.types.get(ty).map(|t| &t.kind),
                    Some(TypeKind::Primitive(_))
                ) {
                    return Ok(false);
                }
                continue;
            }
            let ty = self.ty(self.body.places[a.value.place.0 as usize].ty)?;
            let Some(TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments,
            }) = self.types.get(ty).map(|t| &t.kind)
            else {
                return Ok(false);
            };
            let [GenericArgument::Type(element)] = arguments.as_ref() else {
                return Ok(false);
            };
            if !matches!(
                self.types.get(*element).map(|t| &t.kind),
                Some(TypeKind::Primitive(Primitive::U8))
            ) {
                return Ok(false);
            }
            writers += 1;
        }
        Ok(writers == 1)
    }
}
