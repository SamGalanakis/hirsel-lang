use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    // Terminators.

    /// The one block every return of a register-result entry branches to. Status and
    /// word meet in scalar phis and the pair is built once, so neither return merging
    /// nor the inliner forms a phi of pairs: after inlining, a caller's status test
    /// sees each return site's constant and threads, as it does for a bare status.
    pub(super) fn register_return(
        &mut self,
        kind: ScalarKind,
    ) -> (BasicBlock<'m>, Value<'m>, Value<'m>) {
        if let Some(found) = self.register_return {
            return found;
        }
        let block = self.module.append_block(self.function);
        let builder = Builder::new(self.module);
        builder.position(block);
        let status = builder.phi(self.module.int_type(8));
        let word = builder.phi(value_word(self.module, kind));
        let returned = self.module.const_zero(self.signature.returns);
        let returned = builder.insert(returned, status, self.signature.status_field);
        let (_, field) = self.signature.value.expect("a register result");
        builder.ret(builder.insert(returned, word, field));
        self.register_return = Some((block, status, word));
        (block, status, word)
    }

    /// Returns `status` and, on success with a register result, `value`.
    pub(super) fn return_status(&mut self, status: u128, value: Option<Value<'m>>) -> Emitted {
        let status_byte = self.module.const_int(self.module.int_type(8), status);
        let mut returned = self.module.const_zero(self.signature.returns);
        if let (true, Some((kind, _))) = (self.signature.roots.is_empty(), self.signature.value) {
            let ty = value_word(self.module, kind);
            let word = match value {
                Some(value) => encode_word(&self.builder, kind, value, ty),
                None => self.module.const_zero(ty),
            };
            let (block, status_phi, word_phi) = self.register_return(kind);
            let here = self.builder.current();
            self.builder.br(block);
            self.builder.add_incoming(status_phi, status_byte, here);
            self.builder.add_incoming(word_phi, word, here);
            return Ok(());
        }
        if self.signature.roots.is_empty() {
            self.builder.ret(
                self.signature
                    .returned(&self.builder, returned, status_byte, value),
            );
            return Ok(());
        }
        for (field, &i) in self.signature.roots.iter().enumerate() {
            let source = self.signature.abi.parameters[i].source;
            let slot = self
                .unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(source))
                .and_then(|s| self.slots[s])
                .ok_or_else(|| CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: "missing returned root slot".into(),
                })?;
            let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), slot);
            let (unique, length) = if let Some(state) = self.sequence_state_at(slot) {
                (
                    self.builder.load(self.module.int_type(1), state.unique, 1),
                    self.builder.load(self.module.int_type(64), state.length, 8),
                )
            } else {
                (
                    self.module.const_zero(self.module.int_type(1)),
                    if self.sequence_element(slot.layout)?.is_some() {
                        self.sequence_length(pointer, slot.layout)?
                    } else {
                        self.const_u64(0)
                    },
                )
            };
            returned = self.builder.insert(returned, pointer, field as u32 * 2);
            let Some(index) = self.signature.uniques[field] else {
                returned = self.builder.insert(
                    returned,
                    self.const_u64(status as u64),
                    field as u32 * 2 + 1,
                );
                continue;
            };
            returned = self.builder.insert(returned, unique, index);
            returned = self.builder.insert(returned, length, field as u32 * 2 + 1);
        }
        if self.signature.sequence_roots[0] {
            returned = self.builder.insert(
                returned,
                self.const_u64(status as u64),
                self.signature.status_field,
            );
        }
        self.builder.ret(
            self.signature
                .returned(&self.builder, returned, status_byte, value),
        );
        Ok(())
    }

    pub(super) fn terminator(&mut self, terminator: &Terminator) -> Emitted {
        match *terminator {
            Terminator::Jump(edge) => self.jump(edge),
            Terminator::Branch { condition, yes, no } => {
                let condition = self.get(condition)?;
                let yes = self.edge_block(yes)?;
                let no = self.edge_block(no)?;
                self.builder.cond_br(condition, yes, no);
                Ok(())
            }
            Terminator::Switch {
                value,
                arms,
                otherwise,
            } => {
                // A switch on a variant value selects by its active ordinal: the value is
                // spilled and its tag read as a place's is.
                let layout = self.value_type(value)?.layout;
                let record = self.layout(layout)?;
                let (ty, on) = match (self.value_shape(value)?, &record.tag) {
                    (Shape::Bytes { .. }, Some(tag)) => {
                        let place = self.stack(layout)?;
                        self.store(self.value_shape(value)?, self.get(value)?, place);
                        let on = load_tag(
                            &self.builder,
                            place.ptr,
                            place.align,
                            tag.offset,
                            tag.form,
                            false,
                        );
                        (self.module.int_type(32), on)
                    }
                    _ => {
                        let kind = self.scalar_kind(value)?;
                        let Some((width, _)) = scalar::integer(kind) else {
                            return self.invalid("a switch on a value that is not an integer");
                        };
                        (self.module.int_type(width), self.get(value)?)
                    }
                };
                let mut cases = Vec::new();
                for arm in self.pool(arms, &self.unit.pools.switch_arms)? {
                    cases.push((
                        self.module.const_int(ty, u128::from(arm.value)),
                        self.edge_block(arm.edge)?,
                    ));
                }
                let otherwise = self.edge_block(otherwise)?;
                self.builder.switch(on, otherwise, &cases);
                Ok(())
            }
            Terminator::Call {
                call,
                normal,
                failure,
            } => self.call(call, normal, failure),
            Terminator::Return(value) => {
                self.flush_on_return()?;
                if self.signature.value.is_some() {
                    let Some(value) = value else {
                        return self.invalid("a return without the entry's result");
                    };
                    return self.return_status(0, Some(self.get(value)?));
                }
                if let (Some(value), Some(index)) = (
                    value,
                    self.signature.hidden_index(|h| matches!(h, Hidden::Result)),
                ) {
                    let Some(result) = &self.signature.abi.result else {
                        return self.invalid("a result slot without a result");
                    };
                    let align = self.layout(result.layout)?.align;
                    let out = Place {
                        ptr: self.module.parameter(self.function, index),
                        layout: result.layout,
                        align,
                        block: None,
                    };
                    self.store(self.value_shape(value)?, self.get(value)?, out);
                }
                self.return_status(0, None)?;
                Ok(())
            }
            Terminator::Raise(failure) => {
                let block = self.failure_block(failure)?;
                self.builder.br(block);
                Ok(())
            }
            Terminator::Propagate => {
                self.flush_on_return()?;
                self.return_status(1, None)?;
                Ok(())
            }
            Terminator::InvariantFault { .. } => {
                let fatal = self.runtime(RuntimeEntry::InvariantFault)?;
                self.builder.call(fatal, &[]);
                self.builder.unreachable();
                Ok(())
            }
            Terminator::Unreachable => {
                // Proved unreachable by the validator; still a trap rather than an
                // optimizer assumption.
                let trap = match self.module.intrinsic("llvm.trap", &[]) {
                    Ok(trap) => trap,
                    Err(what) => return self.invalid(what),
                };
                self.builder.call(trap, &[]);
                self.builder.unreachable();
                Ok(())
            }
        }
    }

    /// The link name of function `symbol`, imported at `abi`.
    pub(super) fn named_function(&mut self, symbol: SymbolId, abi: &FnAbi) -> Emitted<String> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a callee outside the symbol table");
        };
        if symbol.kind != SymbolKind::Function {
            return self.invalid("a call of a symbol that is not a function");
        }
        let name = symbol_name(&symbol.definition, symbol.instance.as_ref());
        self.manifested
            .imports
            .entry(name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(symbol.definition.clone()),
                abi: Some(abi_digest(abi)),
            });
        Ok(name)
    }

    pub(super) fn call(
        &mut self,
        call: CallId,
        normal: Option<EdgeId>,
        failure: FailureId,
    ) -> Emitted {
        let unit = self.unit;
        let Some(site) = unit.tables.calls.get(call.index()) else {
            return self.invalid("a call outside the call table");
        };
        let abi = fn_abi(self.inputs.layouts, site.abi, self.definition())?;
        let arguments = self.pool(site.arguments, &unit.pools.call_arguments)?;
        // The callee: a named function, or a code pointer and its environment.
        let (callee, environment, name): (Value<'m>, Option<Value<'m>>, Option<String>) = match site
            .callee
        {
            Callee::Direct(symbol) => (
                self.module.const_zero(self.module.ptr_type()),
                None,
                Some(self.named_function(symbol, abi)?),
            ),
            // A bound call (LIR schema revision 9): the named instance with
            // its closure's environment.
            Callee::Bound { code, closure } => {
                if !abi.environment {
                    return self.invalid("a bound call through an ABI without an environment");
                }
                let (_, env) = self.callable_parts(closure)?;
                (
                    self.module.const_zero(self.module.ptr_type()),
                    Some(env),
                    Some(self.named_function(code, abi)?),
                )
            }
            Callee::Operation(op) => {
                let Some(id) = unit.tables.operations.get(op.index()) else {
                    return self.invalid("an operation outside the operation table");
                };
                let Some(row) = self.inputs.manifest.row(id) else {
                    return self.invalid("an operation the manifest does not list");
                };
                match row.dispositions.get(Engine::Native) {
                    // A runtime family's template names the symbol for this call's scalar
                    // kinds (LIR schema revision 6); a scalar intrinsic is an instruction at
                    // Ready, never a call.
                    Disposition::RuntimePrimitive(template) => {
                        let Some(symbol) =
                            hirsel_lir::abi::resolve_call_symbol(template, unit, site)
                        else {
                            return self.invalid(format!(
                                "runtime template `{template}` names no symbol for this call"
                            ));
                        };
                        self.manifested
                            .imports
                            .entry(symbol.clone())
                            .or_insert_with(|| ObjectSymbol {
                                name: symbol.clone(),
                                kind: ObjectSymbolKind::Runtime,
                                definition: Some(id.definition.clone()),
                                abi: Some(abi_digest(abi)),
                            });
                        (
                            self.module.const_zero(self.module.ptr_type()),
                            None,
                            Some(symbol.clone()),
                        )
                    }
                    other => {
                        return self.unsupported(format!(
                            "an operation with native disposition {other:?}"
                        ));
                    }
                }
            }
            Callee::Indirect(callable) => {
                if !abi.environment {
                    return self.invalid("an indirect call through an ABI without an environment");
                }
                let (code, env) = self.callable_parts(callable)?;
                (code, Some(env), None)
            }
        };
        let root_name = name
            .as_ref()
            .map(|n| format!("{n}{ROOTS_SUFFIX}"))
            .filter(|n| self.manifested.library.contains(n));
        let signature = if root_name.is_some() {
            Signature::of(self.module, self.inputs.layouts, abi, self.definition())?
                .with_roots(self.module, self.inputs.layouts)
        } else if let Callee::Operation(_) = site.callee {
            Signature::runtime(self.module, abi, self.definition())?
        } else {
            Signature::of(self.module, self.inputs.layouts, abi, self.definition())?
        };
        let name = root_name.or(name);
        let function = match &name {
            Some(name) => Some(signature.declare(self.module, name, self.definition(), false)?),
            None => None,
        };
        let mut args = Vec::new();
        let mut result_place = None;
        for hidden in &signature.hidden {
            match hidden {
                Hidden::Context => args.push(self.context),
                Hidden::Environment => match environment {
                    Some(env) => args.push(env),
                    None => return self.invalid("an environment argument without a callable"),
                },
                Hidden::Destination => {
                    let destination = match &site.destination {
                        Some(DestinationClass::Heap) => {
                            self.module.const_zero(self.module.ptr_type())
                        }
                        Some(DestinationClass::Caller(_)) => {
                            match self
                                .signature
                                .hidden_index(|h| matches!(h, Hidden::Destination))
                            {
                                Some(index) => self.module.parameter(self.function, index),
                                None => {
                                    return self
                                        .invalid("a caller destination in a function without one");
                                }
                            }
                        }
                        Some(other) => {
                            return self.unsupported(format!("a call destination {other:?}"));
                        }
                        None => return self.invalid("a destination slot without a destination"),
                    };
                    args.push(destination);
                }
                Hidden::Result => {
                    let Some(result) = &abi.result else {
                        return self.invalid("a result slot without a result");
                    };
                    let place = self.stack(result.layout)?;
                    args.push(place.ptr);
                    result_place = Some(place);
                }
            }
        }
        let mut root_places = Vec::new();
        for (i, parameter) in abi.parameters.iter().enumerate() {
            let Some(argument) = arguments.iter().find(|a| a.parameter == parameter.source) else {
                return self.invalid("a call without an argument for a parameter");
            };
            match (parameter.passing, argument.value) {
                (ParameterPassing::ByValue, ArgumentValue::Value(value)) => {
                    let slots = self.disassemble(parameter.layout, self.get(value)?)?;
                    if slots.len() != parameter.slots.len() {
                        return self.invalid("an argument's slots differ from its parameter's");
                    }
                    args.extend(slots);
                }
                (
                    ParameterPassing::ReadOnlyAddress | ParameterPassing::TransferAddress,
                    ArgumentValue::Value(value),
                ) => {
                    // The callee reads (or takes) the value from caller storage.
                    let place = self.stack(parameter.layout)?;
                    self.store(self.value_shape(value)?, self.get(value)?, place);
                    args.push(place.ptr);
                }
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { address, .. }) => {
                    let place = self.place(address)?;
                    if signature.roots.contains(&i) {
                        args.push(self.load(Shape::Scalar(ScalarKind::Pointer), place));
                        root_places.push(place);
                    } else {
                        args.push(place.ptr);
                    }
                }
                (ParameterPassing::PointerLength, ArgumentValue::Value(value)) => {
                    let place = match self.layout(parameter.layout)?.kind {
                        LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                            if self.value_type(value)?.repr == ValueRepr::Uncounted {
                                self.uncounted_payload(self.get(value)?, payload)?
                            } else {
                                self.payload(self.get(value)?, parameter.layout)?
                            }
                        }
                        LayoutKind::Flat => {
                            let place = self.stack(parameter.layout)?;
                            self.store(self.value_shape(value)?, self.get(value)?, place);
                            place
                        }
                        _ => return self.invalid("a pointer/length argument of a non-flat layout"),
                    };
                    args.push(place.ptr);
                    args.push(self.extent(place.layout, place.ptr)?);
                }
                _ => return self.invalid("an argument passed against its parameter's convention"),
            }
        }
        for (field, place) in root_places.iter().copied().enumerate() {
            if signature.companions[field].is_some() {
                let i1 = self.module.int_type(1);
                // A place with a sequence state defers the count store; any other keeps
                // its count in its block, so the callee stores what it sets.
                let (length, unique, defer) = if let Some(state) = self.sequence_state_at(place) {
                    (
                        self.builder.load(self.module.int_type(64), state.length, 8),
                        self.builder.load(i1, state.unique, 1),
                        self.module.const_int(i1, 1),
                    )
                } else {
                    let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                    (
                        self.sequence_length(pointer, place.layout)?,
                        self.module.const_zero(i1),
                        self.module.const_zero(i1),
                    )
                };
                args.extend([length, unique, defer]);
            }
        }
        // A must-unique fact names a block with one holder, this frame's
        // unescaped slot. A callee can change that count only through a
        // reference it is given: an environment, a caller destination, or a
        // parameter or result whose layout holds one. Private root bodies
        // cannot mutate unrelated, unescaped scalar roots, and a call handed
        // no reference at all (`step(h: u64, value: u64) -> u64`) reaches no
        // counted block; counts are task-local (OWN-6). Every other call
        // retains the conservative invalidation of must-unique facts.
        let plain = |layout: LayoutId| {
            self.inputs
                .layouts
                .layout(layout)
                .is_some_and(|l| l.scan.is_empty() && l.drop.is_empty() && l.finalizer.is_none())
        };
        let isolated_roots = !signature.roots.is_empty()
            && !abi.environment
            && !abi.destination
            && abi.result.is_none()
            && abi
                .parameters
                .iter()
                .enumerate()
                .all(|(i, parameter)| signature.roots.contains(&i) || plain(parameter.layout));
        let referenceless = !abi.environment
            && !abi.destination
            && abi.parameters.iter().all(|p| plain(p.layout))
            && abi.result.as_ref().is_none_or(|r| plain(r.layout));
        if !isolated_roots && !referenceless {
            self.invalidate_uniqueness();
        }
        // A callee reads a deferred slot's count only through what it is handed:
        // the slot's address outside a root parameter (whose companion carries the
        // length), or a view of a borrowed slot in a parameter that can hold one,
        // an environment or a destination. Recursion and insertion helpers handed
        // the list as a root and other data keep the deferral.
        let opaque =
            abi.environment
                || abi.destination
                || abi.parameters.iter().enumerate().any(|(i, parameter)| {
                    !signature.roots.contains(&i) && !plain(parameter.layout)
                });
        let mut handed = vec![false; self.deferred.len()];
        for (i, parameter) in abi.parameters.iter().enumerate() {
            if signature.roots.contains(&i) {
                continue;
            }
            if let Some(argument) = arguments.iter().find(|a| a.parameter == parameter.source)
                && let ArgumentValue::Address { address, .. } = argument.value
                && let Some(slot) = self.address_slot(address)
            {
                handed[slot.index()] = true;
            }
        }
        for slot in 0..self.deferred.len() {
            if handed[slot] || (opaque && self.viewed[slot]) {
                self.flush_length(slot)?;
            }
        }
        let returned = match function {
            Some(function) => self.builder.call(function, &args),
            None => self.builder.call_typed(signature.ty, callee, &args),
        };
        signature.attribute_call(self.module, returned);
        // Hand back every root before branching on status: cleanup and Error observe
        // exactly the same callee mutations as successful continuation.
        let status = signature.status(&self.builder, returned);
        if !signature.roots.is_empty() {
            let succeeded = self.builder.icmp(
                IntPredicate::Eq,
                status,
                self.module.const_int(self.module.int_type(8), 0),
            );
            for (field, place) in root_places.into_iter().enumerate() {
                let pointer = signature.root_pointer(&self.builder, returned, field);
                self.store(Shape::Scalar(ScalarKind::Pointer), pointer, place);
                if let Some(state) = self.sequence_state_at(place) {
                    self.builder.store(
                        self.builder.extract(returned, field as u32 * 2 + 1),
                        state.length,
                        8,
                    );
                    self.builder.store(
                        self.module.const_int(self.module.int_type(1), 1),
                        state.dirty,
                        1,
                    );
                    // list_grow's SeqReserve hands back unique storage. Its
                    // cold body stays out of line, so LLVM cannot recover this
                    // fact from the tagged aggregate return at the call site.
                    let reserved = matches!(site.callee, Callee::Direct(symbol)
                        if self.unit.tables.symbols[symbol.index()].definition.is_core("$native.list_grow"));
                    let unique = if reserved {
                        succeeded
                    } else {
                        signature.root_unique(&self.builder, returned, field)
                    };
                    self.builder.store(unique, state.unique, 1);
                }
            }
        }
        let ok = self.builder.icmp(
            IntPredicate::Eq,
            status,
            self.module.const_int(self.module.int_type(8), 0),
        );
        let failure_block = self.failure_block(failure)?;
        let fail = if self.unit.header.origin == UnitOrigin::Library {
            failure_block
        } else {
            let block = self.module.append_block(self.function);
            let here = self.builder.current();
            self.builder.position(block);
            let reanchor = self.runtime(RuntimeEntry::FailureReanchor)?;
            let anchor = self.const_u64(self.anchor_code(site.anchor)?);
            self.builder.call(reanchor, &[self.context, anchor]);
            self.builder.br(failure_block);
            self.builder.position(here);
            block
        };
        let fail = if let Callee::Operation(op) = site.callee
            && let Some(rule) = self
                .inputs
                .manifest
                .row(&unit.tables.operations[op.index()])
                .and_then(|row| row.failure.as_ref())
        {
            let Some(code) = self.inputs.failures.codes.get(&rule.0).copied() else {
                return Err(CodegenError::UnknownRule {
                    definition: self.definition().clone(),
                    rule: rule.0.clone(),
                });
            };
            if self.failure_edge(failure)?.rule.is_some() {
                return self.invalid("a runtime call failure rule must be recorded exactly once");
            }
            let here = self.builder.current();
            let record = self.module.append_block(self.function);
            self.builder.position(record);
            let begin = self.runtime(RuntimeEntry::FailureBegin)?;
            let site = self.anchor_code(site.anchor)?;
            let i32_ty = self.module.int_type(32);
            self.builder.call(
                begin,
                &[
                    self.context,
                    self.module.const_int(i32_ty, u128::from(code.kind)),
                    self.module.const_int(i32_ty, u128::from(code.rule)),
                    self.const_u64(site),
                ],
            );
            self.builder.br(fail);
            self.builder.position(here);
            record
        } else {
            fail
        };
        let next = self.module.append_block(self.function);
        self.builder.cond_br(ok, next, fail);
        self.builder.position(next);
        // A diverging callee (LIR schema revision 19) reports only failure:
        // its success status is impossible. It is not `noreturn`, since a
        // language failure returns status 1 to this frame's failure edge.
        let Some(normal) = normal else {
            self.builder.unreachable();
            return Ok(());
        };
        if let Some(result) = site.result {
            let value = match (signature.result(&self.builder, returned), result_place) {
                (Some(value), _) => value,
                (None, Some(place)) => self.load(self.value_shape(result)?, place),
                (None, None) => return self.invalid("a call result without a result slot"),
            };
            self.define(result, value)?;
        }
        self.jump(normal)
    }
}
