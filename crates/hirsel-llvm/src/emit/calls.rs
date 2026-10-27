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
        self.ownership_leave()?;
        self.ahead_point(false)?;
        self.ahead_flush();
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
            let (unique, untracked, length) = if let Some(state) = self.sequence_state_at(slot) {
                (
                    self.builder.load(self.module.int_type(1), state.unique, 1),
                    self.builder
                        .load(self.module.int_type(1), state.untracked, 1),
                    self.builder.load(self.module.int_type(64), state.length, 8),
                )
            } else {
                (
                    self.module.const_zero(self.module.int_type(1)),
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
            if let Some(index) = self.signature.untrackeds.get(field).copied().flatten() {
                returned = self.builder.insert(returned, untracked, index);
            }
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
        if self.ahead_state.is_some()
            && self
                .inputs
                .ahead
                .and_then(|p| p.host_spine(self.unit.header.instance))
                .is_some()
            && match terminator {
                Terminator::Jump(_) | Terminator::Call { .. } => false,
                Terminator::Branch {
                    condition: value, ..
                }
                | Terminator::Switch { value, .. } => !hirsel_lir::ahead::scalar_image(
                    self.inputs.layouts,
                    self.unit.types[self.unit.values[value.index()].ty.index()].layout,
                ),
                _ => true,
            }
        {
            self.ahead_barrier()?;
        }
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
                        columns: None,
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
            Terminator::Recover {
                arena,
                caught,
                failed,
            } => self.recover(arena, caught, failed),
            // FAIL-2: the runtime keeps the failures recorded since `start` for the
            // caught code and forgets them; ordinary code resumes.
            Terminator::Isolate { start, caught } => {
                let entry = self.runtime(RuntimeEntry::IsolateCatch)?;
                self.builder.call(entry, &[self.context, self.get(start)?]);
                let caught = self.edge_block(caught)?;
                self.builder.br(caught);
                Ok(())
            }
            Terminator::RejectTransfer => {
                self.return_status(
                    hirsel_lir::runtime_abi::STATUS_PRETRANSFER_FAILURE as u128,
                    None,
                )?;
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
                // optimizer assumption. Source matches with a catch-all instead
                // retain that arm as a defined default in LIR (both native
                // backends); checked H validates the ordinal before dispatch.
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
        let name = if self.inputs.alternate {
            let id = symbol.instance.ok_or_else(|| CodegenError::Invalid {
                definition: self.definition().clone(),
                what: "unknown alternate callee".into(),
            })?;
            if !self
                .inputs
                .ahead
                .is_some_and(|p| p.has_private_edge(self.unit.header.instance, id))
            {
                if self
                    .inputs
                    .ahead
                    .and_then(|p| p.host_spine(self.unit.header.instance))
                    .is_none()
                {
                    return self.invalid("missing real alternate callee");
                }
                symbol_name(&symbol.definition, symbol.instance.as_ref())
            } else {
                hirsel_lir::ahead::alternate_name(&symbol.definition, id)
            }
        } else {
            symbol_name(&symbol.definition, symbol.instance.as_ref())
        };
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
        self.call_with_abi(call, normal, failure, abi)
    }

    /// Lower an already selected ABI. The public unit path above retains its
    /// admission guard; private backend probes can exercise physical transport.
    pub(super) fn call_with_abi(
        &mut self,
        call: CallId,
        normal: Option<EdgeId>,
        failure: FailureId,
        abi: &'a FnAbi,
    ) -> Emitted {
        let unit = self.unit;
        let Some(site) = unit.tables.calls.get(call.index()) else {
            return self.invalid("a call outside the call table");
        };
        self.check_receipt_call(site, abi)?;
        if abi.shape == CallableShape::Foreign {
            return self.foreign_call(call, abi, normal);
        }
        // Any boundary call can suspend its frame: a resumable callee, or an
        // operation that holds or parks from the saved frame (`STATUS_SUSPENDED`).
        // An operation's library body returns its entry's suspension to that
        // boundary call.
        let boundary = self.resume_boundary_call(call);
        let suspends = boundary
            || matches!(site.callee, Callee::Operation(op) if unit
                .tables
                .operations
                .get(op.index())
                .is_some_and(|id| hirsel_lir::continuation::leaves_in_library(&id.definition)));
        let nested = self.resume_boundary(call)?;
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
                let name = self
                    .inputs
                    .ahead
                    .filter(|_| self.inputs.alternate)
                    .and_then(|plan| plan.cpu_mapper(unit.header.instance, call))
                    .map(|symbol| {
                        let name = hirsel_lir::ahead::alternate_name(
                            &symbol.definition,
                            symbol.instance.unwrap(),
                        );
                        self.manifested
                            .imports
                            .entry(name.clone())
                            .or_insert_with(|| ObjectSymbol {
                                name: name.clone(),
                                kind: ObjectSymbolKind::Function,
                                definition: Some(symbol.definition.clone()),
                                abi: Some(abi_digest(abi)),
                            });
                        name
                    });
                (code, Some(env), name)
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
        // A runtime primitive's body is the runtime's, which states nothing.
        let aliasing = match site.callee {
            Callee::Operation(_) => Aliasing::Unstated,
            _ => Aliasing::of(self.inputs.callbacks),
        };
        let function = match &name {
            Some(name) => Some(signature.declare(
                self.module,
                self.inputs.layouts,
                name,
                self.definition(),
                false,
                aliasing,
            )?),
            None => None,
        };
        if name.as_deref() == Some("hirsel_rt_v2_net_read_into")
            && let Some(function) = function
        {
            // The runtime writes only the received prefix and retains no address
            // past this call. Observed suffixes still retain their initializer.
            for attribute in [Attribute::WriteOnly, Attribute::CapturesNone] {
                self.module
                    .add_attribute(function, AttributeSlot::Parameter(5), attribute);
            }
        }
        let mut args = Vec::new();
        let mut result_place = None;
        let codes = self.caller_sites(self.anchor_code(site.anchor)?, site.forward_caller);
        for hidden in &signature.hidden {
            match hidden {
                Hidden::Context => args.push(self.context),
                Hidden::CallerSite => args.push(codes.0),
                Hidden::OperationSite => args.push(codes.1),
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
                    let place = match self.runtime_result(call, result.layout)? {
                        Some(place) => place,
                        None => self.stack(result.layout)?,
                    };
                    args.push(place.ptr);
                    result_place = Some(place);
                }
            }
        }
        let mut root_places = Vec::new();
        let mut temporary_returns = Vec::new();
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
                // A resumable callee's frame holds the address it is passed:
                // only a slot of this frame, proved for this edge.
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { .. })
                    if nested.is_some() =>
                {
                    let Some(proof) = self.resume_exclusive(call, parameter.source) else {
                        return self.invalid(
                            "a nested resumable callee's exclusive parameter without its edge proof",
                        );
                    };
                    if signature.roots.contains(&i) {
                        return self.invalid("an exclusive root parameter of a nested frame");
                    }
                    args.push(self.exclusive_slot_address(proof)?);
                }
                (ParameterPassing::ExclusiveAddress, ArgumentValue::Address { address, .. }) => {
                    let place =
                        match self.runtime_argument(call, parameter.source, parameter.layout)? {
                            Some(place) => place,
                            None => self.place(address)?,
                        };
                    if signature.roots.contains(&i) {
                        args.push(self.load(Shape::Scalar(ScalarKind::Pointer), place));
                        root_places.push(place);
                    } else {
                        let passed = if place.columns.is_some() {
                            self.gather(place)?
                        } else if place.align < self.layout(parameter.layout)?.align {
                            // @packed fields can be underaligned. A Hirsel entry's
                            // address ABI promises the parameter layout's alignment.
                            let temp = self.stack(parameter.layout)?;
                            let shape = self.layout_shape(parameter.layout)?;
                            self.store(shape, self.load(shape, place), temp);
                            temp
                        } else {
                            place
                        };
                        if place.columns.is_some() || place.align < passed.align {
                            temporary_returns.push((place, passed));
                        }
                        args.push(passed.ptr);
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
        let receipt_result = self.receipt_call_arguments(site, &mut args)?;
        // list_grow's SeqReserve hands back unique storage unless it was given a tracked
        // arena block (MEM-6), which it leaves alone: known before the call, or read from it.
        let reserved = matches!(site.callee, Callee::Direct(symbol)
            if self.unit.tables.symbols[symbol.index()].definition.is_core("$native.list_grow"));
        let mut kept_unique = vec![None; root_places.len()];
        for (field, place) in root_places.iter().copied().enumerate() {
            if reserved && let Some(state) = self.sequence_state_at(place) {
                let old = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                kept_unique[field] = Some(self.grown_facts(state, old).0);
            }
        }
        for (field, place) in root_places.iter().copied().enumerate() {
            if signature.companions[field].is_some() {
                let i1 = self.module.int_type(1);
                // A place with a sequence state defers the count store; any other keeps
                // its count in its block, so the callee stores what it sets.
                let (length, unique, defer, untracked) =
                    if let Some(state) = self.sequence_state_at(place) {
                        (
                            self.builder.load(self.module.int_type(64), state.length, 8),
                            self.builder.load(i1, state.unique, 1),
                            self.module.const_int(i1, 1),
                            self.builder.load(i1, state.untracked, 1),
                        )
                    } else {
                        let pointer = self.load(Shape::Scalar(ScalarKind::Pointer), place);
                        (
                            self.sequence_length(pointer, place.layout)?,
                            self.module.const_zero(i1),
                            self.module.const_zero(i1),
                            self.module.const_zero(i1),
                        )
                    };
                args.extend([length, unique, defer, untracked]);
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
        if let Some((root, identity, source, source_abi)) = self.inputs.ahead.and_then(|p| {
            let root = p.cpu_start(unit, call)?;
            let recipe = &p.roots[&root].recipe;
            Some((
                root,
                p.identity,
                recipe.source_code.clone(),
                recipe.source_abi,
            ))
        }) {
            let slot = unit
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(3))
                .and_then(|s| self.slots[s])
                .ok_or_else(|| CodegenError::Invalid {
                    definition: self.definition().clone(),
                    what: "missing CPU mapper slot".into(),
                })?;
            let actual = self.builder.load(self.module.ptr_type(), slot.ptr, 8);
            let name = symbol_name(&source.definition, source.instance.as_ref());
            let abi = fn_abi(self.inputs.layouts, source_abi, self.definition())?;
            self.import_function(&name, &source.definition, Some(abi_digest(abi)));
            Signature::of(self.module, self.inputs.layouts, abi, &source.definition)?.declare(
                self.module,
                self.inputs.layouts,
                &name,
                &source.definition,
                false,
                Aliasing::Unstated,
            )?;
            let expected = self.module.function_address(&name);
            let metadata = self.glue_cell(
                &hirsel_lir::ahead::metadata_name(root, identity),
                self.module.int_type(8),
            );
            let version = self.builder.load(self.module.int_type(32), metadata, 4);
            let certified = self.builder.icmp(
                IntPredicate::Eq,
                version,
                self.module.const_int(self.module.int_type(32), 6),
            );
            let matches = self.builder.icmp(IntPredicate::Eq, actual, expected);
            let matches = self.builder.binary(BinOp::And, certified, matches);
            args[4] = self.builder.select(
                matches,
                self.module.const_int(self.module.int_type(32), 4),
                args[4],
            );
        }
        let ahead_root = self.inputs.ahead.and_then(|p| {
            p.spawns
                .get(&(unit.header.instance, call))
                .map(|root| (*root, p.identity))
        });
        let function = if let Some((root, identity)) = ahead_root {
            let metadata = hirsel_lir::ahead::metadata_name(root, identity);
            args.push(self.glue_cell(&metadata, self.module.int_type(8)));
            Some(self.runtime(RuntimeEntry::TaskSpawnAhead)?)
        } else {
            function
        };
        // A resumable callee continues in its own frame inside this one.
        let function = match (nested, site.callee) {
            (
                Some((frame, callee)),
                Callee::Direct(symbol) | Callee::Bound { code: symbol, .. },
            ) => {
                args.push(frame);
                Some(self.resume_callee(symbol, callee, &signature)?)
            }
            (Some(_), _) => return self.invalid("a nested resumable call without a known callee"),
            (None, _) => function,
        };
        if matches!(site.callee, Callee::Operation(_))
            || self
                .inputs
                .ahead
                .and_then(|p| p.host_spine(unit.header.instance))
                .is_some_and(|s| !s.scalar_calls.contains(&call))
        {
            self.ahead_barrier()?;
        }
        let (function, name) = if let Some(entry) = self.ordered_read(call) {
            // Original read primitives have no hidden source sites. Only the
            // certified helper receives those additional physical arguments.
            args.splice(1..1, [codes.0, codes.1]);
            (
                Some(self.runtime(entry)?),
                Some(entry.declaration().symbol.to_owned()),
            )
        } else if let Some(entry) = self.ordered_host(call) {
            (
                Some(self.runtime(entry)?),
                Some(entry.declaration().symbol.to_owned()),
            )
        } else if let Some(align) = self.ordered_join_align(call) {
            args.push(
                self.module
                    .const_int(self.module.int_type(32), u128::from(align)),
            );
            (
                Some(self.runtime(RuntimeEntry::OrderedJoin)?),
                Some("hirsel_rt_v2_ordered_join".to_owned()),
            )
        } else {
            (function, name)
        };
        self.host_read_boundary(call, false)?;
        self.ahead_flush();
        self.ownership_call(call, false)?;
        self.bind_runtime(call, name.as_deref(), codes)?;
        self.resume_tail_bind(call)?;
        let returned = match function {
            Some(function) => self.builder.call(function, &args),
            None => self.builder.call_typed(signature.ty, callee, &args),
        };
        self.ahead_reload();
        signature.attribute_call(self.module, returned);
        // A rejected gate leaves mut roots and temporary scatters unpublished.
        let status = signature.status(&self.builder, returned);
        if suspends {
            self.resume_propagate(status)?;
        }
        if let Some(rejected) = site.pretransfer_failure {
            let rejected = self.failure_block(rejected)?;
            let entered = self.module.append_block(self.function);
            let pending = self.builder.icmp(
                IntPredicate::Eq,
                status,
                self.module.const_int(
                    self.module.int_type(8),
                    hirsel_lir::runtime_abi::STATUS_PRETRANSFER_FAILURE as u128,
                ),
            );
            self.builder.cond_br(pending, rejected, entered);
            self.builder.position(entered);
        }
        // Publish temporary mutations even when the entered callee failed.
        for (destination, temporary) in temporary_returns {
            let shape = self.layout_shape(destination.layout)?;
            self.scatter(destination, shape, self.load(shape, temporary))?;
        }
        // Hand back every root before branching on status: cleanup and Error observe
        // exactly the same callee mutations as successful continuation.
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
                    let (unique, untracked) = if let Some(kept) = kept_unique[field] {
                        (self.builder.binary(BinOp::And, succeeded, kept), kept)
                    } else {
                        (
                            signature.root_unique(&self.builder, returned, field),
                            signature.root_untracked(&self.builder, returned, field),
                        )
                    };
                    self.builder.store(unique, state.unique, 1);
                    self.builder.store(untracked, state.untracked, 1);
                }
            }
        }
        self.host_read_boundary(call, true)?;
        let ok = self.builder.icmp(
            IntPredicate::Eq,
            status,
            self.module.const_int(self.module.int_type(8), 0),
        );
        let failure_block = self.failure_block(failure)?;
        // An unanchored primitive failure must cross the original function-value
        // boundary before the surrounding marked frame's ordinary failure edge.
        let fail = if site.forward_caller {
            failure_block
        } else {
            let block = self.module.append_block(self.function);
            let here = self.builder.current();
            self.builder.position(block);
            let (caller, operation) = self.failure_sites_for(self.anchor_code(site.anchor)?, false);
            self.ahead_barrier()?;
            let reanchor = self.runtime(RuntimeEntry::FailureReanchor)?;
            self.builder
                .call(reanchor, &[self.context, caller, operation]);
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
            if self.failure_edge(failure)?.rule.is_some() {
                return self.invalid("a runtime call failure rule must be recorded exactly once");
            }
            let here = self.builder.current();
            let record = self.module.append_block(self.function);
            self.builder.position(record);
            self.ahead_barrier()?;
            let begin = self.runtime(RuntimeEntry::FailureBegin)?;
            let (site, operation) =
                self.failure_sites_for(self.anchor_code(site.anchor)?, site.forward_caller);
            let (kind, rule) = self.failure_code(&rule.0);
            self.builder
                .call(begin, &[self.context, kind, rule, site, operation]);
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
        if let Some((result, place)) = receipt_result {
            self.define(result, self.load(Shape::Scalar(ScalarKind::Pointer), place))?;
        }
        if let Some(result) = site.result {
            let value = match (signature.result(&self.builder, returned), result_place) {
                (Some(value), _) => value,
                (None, Some(place)) => self.load(self.value_shape(result)?, place),
                (None, None) => return self.invalid("a call result without a result slot"),
            };
            self.define(result, value)?;
        }
        self.ownership_call(call, true)?;
        self.jump(normal)
    }
}
