use super::*;
use crate::PassError;

impl Importer<'_, '_> {
    pub(super) fn terminator(&mut self, t: tir::BlockId, term: &tir::Terminator) -> R<()> {
        use tir::Terminator as T;
        match term {
            T::Goto(next) => {
                let e = self.enter(*next);
                self.terminate(Terminator::Jump(e));
            }
            T::Branch { condition, yes, no } => {
                let c = self.operand(condition)?;
                let yes = self.enter(*yes);
                let no = self.enter(*no);
                self.terminate(Terminator::Branch {
                    condition: c,
                    yes,
                    no,
                });
            }
            T::Switch {
                value,
                arms,
                otherwise,
            } => {
                let v = self.operand(value)?;
                let arms: Vec<SwitchArm> = arms
                    .iter()
                    .map(|(value, b)| SwitchArm {
                        value: *value,
                        edge: self.enter(*b),
                    })
                    .collect();
                let otherwise = self.enter(*otherwise);
                let arms = PoolRange::push(&mut self.out.pools.switch_arms, arms);
                self.terminate(Terminator::Switch {
                    value: v,
                    arms,
                    otherwise,
                });
            }
            T::Unreachable => self.terminate(Terminator::Unreachable),
            T::Return { value, cleanup } => {
                // Extracting the result may fail (a copy): the return's cleanup runs.
                self.failure.cleanup = Some(*cleanup);
                let v = self.operand(value)?;
                // A many-call closure returned as a once-call one (MODE-3).
                let v = match self.own_abi().result.as_ref().map(|r| r.layout) {
                    Some(layout) => {
                        let ty = self.intern(layout);
                        self.weaken(v, ty)
                    }
                    None => v,
                };
                let actions = self.exit_actions(self.chain_actions(*cleanup)?);
                self.failure = FailureContext {
                    cleanup: None,
                    before: Vec::new(),
                    pending: if self.holds(v) { vec![v] } else { Vec::new() },
                };
                self.run_normal(&actions, &[])?;
                let abi = self.own_abi();
                let result = abi.result.is_some().then_some(v);
                self.terminate(Terminator::Return(result));
            }
            T::Fail { rule, cleanup } => {
                self.failure = FailureContext {
                    cleanup: Some(*cleanup),
                    before: Vec::new(),
                    pending: Vec::new(),
                };
                let failure = self.fail(Some(rule.to_string()))?;
                self.terminate(Terminator::Raise(failure));
            }
            T::Call {
                callee,
                arguments,
                returns,
                on_failure,
            } => {
                self.failure = FailureContext {
                    cleanup: Some(*on_failure),
                    before: Vec::new(),
                    pending: Vec::new(),
                };
                if self.builders.calls.contains(&t) {
                    return self.removed_call(*returns);
                }
                if self.builders.reserves.contains(&t) {
                    return self.builder_reserve(arguments, *returns);
                }
                if let Some(&write) = self.fields.writes.get(&t) {
                    return self.field_write(write, arguments, *returns);
                }
                if self.fields.freezes.contains(&t) {
                    return self.field_freeze(*returns);
                }
                if let Some(site) = self.plan.and_then(|p| p.site(t)).cloned() {
                    return self.back_edge(&site, arguments);
                }
                self.call_block = t;
                self.call(callee, arguments, *returns)?;
            }
            T::Match { table, from } => self.match_from(*table, *from)?,
        }
        Ok(())
    }

    /// An `ArenaPush` of the read's maximum through an address of its own
    /// of its `mut` buffer, a separate write P4 leaves to it.
    fn read_room(&mut self, arguments: &[tir::CallArgument], buffer: u32, max: u32) -> R<()> {
        let argument = |at: u32| {
            arguments
                .iter()
                .find(|a| a.parameter.0 == at)
                .ok_or_else(|| PassError::Malformed("an Io read's arguments".into()))
        };
        let (buffer, max) = (argument(buffer)?, argument(max)?);
        if buffer.value.usage != UseKind::Mut {
            return malformed("an Io read's buffer is not lent mut");
        }
        let count = self.operand(&max.value)?;
        self.append_room(buffer.value.place, count)
    }

    fn append_room(&mut self, buffer: tir::PlaceId, count: ValueId) -> R<()> {
        let p = self.place(buffer)?;
        let address = self.address(&p)?;
        let ty = self.out.slots[p.slot.index()].ty;
        let version = self.value(ty, Holding::Plain);
        let failure = self.fail(None)?;
        self.push(Inst::ArenaPush {
            address,
            count: Some(count),
            version,
            failure,
        });
        self.versions[p.slot.index()] = Some(version);
        Ok(())
    }

    pub(super) fn own_abi(&self) -> hirsel_lir::abi_types::FnAbi {
        self.layouts.table().fn_abis[self.instances.abis[self.id.index()].0 as usize].clone()
    }

    pub(super) fn call(
        &mut self,
        callee: &tir::Callee,
        arguments: &[tir::CallArgument],
        returns: Option<tir::CallReturn>,
    ) -> R<()> {
        // `list.len` of a list inside a flat value reads its encoding's count
        // in place: a flat list has no ordinary List to lend (LIR schema
        // revision 4).
        if let tir::Callee::Definition { path, .. } = callee
            && let Some(tir::CallReturn { destination, next }) = returns
            && self.program.operations.contains_key(path)
            && core_operation(path, "list", "len")
            && let [argument] = arguments
            && argument.value.usage == UseKind::Borrow
            && self.joined_view(&argument.value).is_none()
        {
            let p = self.place(argument.value.place)?;
            if p.source.is_some() {
                return self.flat_length(&p, destination, next);
            }
        }
        if let tir::Callee::Definition { path, .. } = callee
            && path.is_core("string.as_bytes")
            && let Some(ret) = returns
            && self.aliases[self.body.places[ret.destination.0 as usize].root.0 as usize].is_some()
        {
            let edge = self.enter(ret.next);
            self.terminate(Terminator::Jump(edge));
            return Ok(());
        }
        // A family's inline lowering replaces the call by instructions.
        if let tir::Callee::Definition {
            path,
            arguments: generics,
        } = callee
            && let Some(tir::CallReturn { destination, next }) = returns
            && let Some(lowering) = crate::library::inline_lowering(path)
        {
            let shape = crate::library::CallShape {
                path,
                generics,
                arguments,
                destination,
                next,
            };
            if (lowering.lower)(&mut ImportCx { im: self }, &shape)? {
                return Ok(());
            }
        }
        // MEM-6: an Io read's appends are charged and given room in an arena
        // List before its effect and before its operands, so an `OutOfSpace`
        // reads nothing (LIR schema revision 34). It takes no input.
        if let tir::Callee::Definition { path, .. } = callee
            && self.program.operations.contains_key(path)
            && let Some((buffer, max)) = read_room(path)
        {
            self.read_room(arguments, buffer, max)?;
        }
        if let tir::Callee::Definition { path, .. } = callee
            && path.is_core("bytes.append")
        {
            let builder = arguments
                .iter()
                .find(|a| a.parameter.0 == 0)
                .ok_or_else(|| PassError::Malformed("byte append destination".into()))?;
            let data = arguments
                .iter()
                .find(|a| a.parameter.0 == 1)
                .ok_or_else(|| PassError::Malformed("byte append source".into()))?;
            let value = self.operand(&data.value)?;
            let ty = self.value_type(value);
            let word = self.record(ty).fields[1].layout;
            let word = self.intern(word);
            let count = self.value(word, Holding::Plain);
            self.push(Inst::SeqLen {
                result: count,
                value,
            });
            // An empty append neither prepares nor charges shared storage.
            // Both paths return through the original continuation, whose slot
            // versions therefore join in the ordinary importer SSA machinery.
            let ret =
                returns.ok_or_else(|| PassError::Malformed("byte append continuation".into()))?;
            self.tir_preds[ret.next.0 as usize] += 1;
            let zero_constant = self.constant(word, ConstantValue::Integer(0));
            let zero = self.value(word, Holding::Plain);
            self.push(Inst::Const {
                result: zero,
                constant: zero_constant,
            });
            let boolean = self.bool_type()?;
            let empty = self.value(boolean, Holding::Plain);
            let failure = self.fail(None)?;
            self.push(Inst::Binary {
                result: empty,
                op: BinaryOp::Eq,
                left: count,
                right: zero,
                failure,
            });
            let done = self.block(BlockKind::Body);
            let append = self.block(BlockKind::Body);
            let yes = self.edge(done, Vec::new());
            let no = self.edge(append, Vec::new());
            self.terminate(Terminator::Branch {
                condition: empty,
                yes,
                no,
            });
            let state = self.state.clone();
            let versions = self.versions.clone();
            let views = self.views.clone();
            let facts = self.facts.clone();
            self.current = done;
            self.removed_call(returns)?;
            self.state = state;
            self.versions = versions;
            self.views = views;
            self.facts = facts;
            self.current = append;
            self.append_room(builder.value.place, count)?;
        }
        // Native operand preparation can still fail before the call is
        // entered. Its TIR failure chain already excludes all taken inputs;
        // until they transfer, clean the storage actually owned here instead.
        let entered_cleanup = self.failure.cleanup.take();
        let mut taken_callee: Option<(LoanId, PlaceRef)> = None;
        let mut capture_args = Vec::new();
        let mut capture_loans = Vec::new();
        let mut operation_arguments = Vec::new();
        let (lir_callee, abi) = match callee {
            tir::Callee::Definition { path, arguments } => {
                if let Some(body) = crate::library::body_index(path) {
                    // A library body: a direct call of its instance.
                    let request = self.library_request(body, path, arguments)?;
                    let direct = self.require(request)?;
                    let symbol = self.symbol(
                        SymbolKind::Function,
                        &direct.symbol.definition,
                        direct.symbol.instance,
                    );
                    (Callee::Direct(symbol), direct.abi)
                } else if let Some(op) = self.program.operations.get(path).cloned() {
                    let reference = self.operation(&op);
                    let (abi, actuals) = self.operation_abi(path, arguments)?;
                    operation_arguments = actuals;
                    (Callee::Operation(reference), abi)
                } else if self.program.definitions.contains_key(path)
                    || self.program.instance_bodies.declarations.contains(path)
                {
                    let (symbol, abi) = self.direct(path, arguments)?;
                    (Callee::Direct(symbol), abi)
                } else if let Some(abi) = self.foreign_abi(path)? {
                    let symbol = self.symbol(SymbolKind::Function, path, None);
                    (Callee::Direct(symbol), abi)
                } else {
                    return unsupported(format!(
                        "a call of {path:?}, neither an operation nor a definition"
                    ));
                }
            }
            tir::Callee::Indirect {
                forward_caller: _,
                value,
                arguments: generics,
            } => {
                // Generic nested names call concrete code with only their
                // declaration's ordinary captured data (spec #42).
                let bound = if generics.is_empty() {
                    None
                } else {
                    let Some(&instance) =
                        self.instances.direct_calls.get(&(self.id, self.call_block))
                    else {
                        return Err(InstanceError::MissingInstance(self.def.path.clone()).into());
                    };
                    let digest = self.instances.view(instance).key.digest();
                    let code =
                        self.symbol(SymbolKind::Function, &self.def.path.clone(), Some(digest));
                    Some((code, self.instances.abis[instance.index()]))
                };
                let abi = match bound {
                    Some((_, abi)) => abi,
                    None => {
                        let ty = self.ty(self.body.places[value.place.0 as usize].ty)?;
                        self.layouts
                            .fn_abi(&self.store, ty, CallableShape::Closure)
                            .map_err(|e| PassError::Layout(self.def.path.clone(), e))?
                    }
                };
                if let Some((code, _)) = bound {
                    if let Some(&instance) =
                        self.instances.direct_calls.get(&(self.id, self.call_block))
                        && let Some(captures) = self.instances.direct_captures.get(&instance)
                    {
                        let p = self.place(value.place)?;
                        let base = self.layouts.table().fn_abis[abi.0 as usize]
                            .parameters
                            .len()
                            - captures.len();
                        for (ordinal, capture) in captures.iter().enumerate() {
                            let mut field = p.clone();
                            field.steps.push(Step::Field(ordinal as u32));
                            field.projection.push(ProjectionStep::Field(ordinal as u32));
                            field.ty = self.intern(capture.layout);
                            let address = self.address(&field)?;
                            let loan = match p.loan {
                                Some(loan) => self.loan(loan)?,
                                None => {
                                    let loan = self.synthetic_loan(address);
                                    capture_loans.push(loan);
                                    loan
                                }
                            };
                            let holding = if self.owns(field.ty) {
                                Holding::Borrowed(loan)
                            } else {
                                Holding::Plain
                            };
                            let result = self.value(field.ty, holding);
                            let reaching = self.reaching(p.slot)?;
                            self.push(Inst::Read {
                                result,
                                address,
                                reaching,
                                mode: ReadMode::Borrow,
                            });
                            let argument = ArgumentValue::Value(result);
                            capture_args.push(CallArgument {
                                parameter: (base + ordinal) as u32,
                                passing: Passing::Borrow,
                                value: argument,
                            });
                        }
                    }
                    (Callee::Direct(code), abi)
                } else {
                    let mut p = self.place(value.place)?;
                    let v = if value.usage == UseKind::Take
                        && self.owns(p.ty)
                        && !self.borrowed_storage(p.slot)
                    {
                        // Extraction creates the invocation's owner after the
                        // argument temporaries. Borrow that owner for the call,
                        // then release it before the caller's cleanup. Keeping it
                        // in its earlier slot would reverse temporary drop order.
                        if !p.fixed {
                            return unsupported("a taken callee in a computed place");
                        }
                        let owned = self.operand(value)?;
                        // A diverging call's owner lives in its callee's scope.
                        let scope = match returns {
                            Some(r) => {
                                let root = self.body.places[r.destination.0 as usize].root;
                                self.out.slots[self.slot_for(root.0 as usize)?.index()].scope
                            }
                            None => self.out.slots[p.slot.index()].scope,
                        };
                        let slot = self.new_slot(p.ty, SlotKind::Temporary, scope);
                        self.push(Inst::StorageLive { slot });
                        self.state.live(slot);
                        p = self.whole(slot);
                        let address = self.address(&p)?;
                        self.store(&p, address, owned)?;
                        let loan = self.synthetic_loan(address);
                        let reaching = self.reaching(p.slot)?;
                        let v = self.value(p.ty, Holding::Borrowed(loan));
                        self.push(Inst::Read {
                            result: v,
                            address,
                            reaching,
                            mode: ReadMode::Borrow,
                        });
                        taken_callee = Some((loan, p));
                        v
                    } else {
                        self.operand(value)?
                    };
                    (Callee::Indirect(v), abi)
                }
            }
        };
        let release = taken_callee.as_ref().map(|(loan, p)| {
            [
                Action::EndLoan { loan: *loan },
                Action::Release {
                    slot: p.slot,
                    anchor: (INSTRUCTION_DROP, self.statement_action()),
                },
            ]
        });
        if let Some(release) = &release {
            self.failure.before.splice(0..0, release.iter().cloned());
        }
        let fn_abi = self.layouts.table().fn_abis[abi.0 as usize].clone();
        let mut args = capture_args;
        let mut writes = Vec::new();
        for a in arguments {
            let passing = fn_abi
                .parameters
                .iter()
                .find(|p| p.source == a.parameter.0)
                .map(|p| (p.convention, p.layout));
            let Some((passing, layout)) = passing else {
                return malformed(format!(
                    "argument for undeclared parameter {}",
                    a.parameter.0
                ));
            };
            let value = if a.value.usage == UseKind::Mut {
                let p = self.place(a.value.place)?;
                let address = self.address(&p)?;
                let ty = self.out.slots[p.slot.index()].ty;
                let version = self.value(ty, Holding::Plain);
                writes.push((p.slot, version));
                ArgumentValue::Address { address, version }
            } else {
                let value = self.operand(&a.value)?;
                let value = if matches!(passing, Passing::Take | Passing::Borrow) {
                    let ty = self.intern(layout);
                    self.weaken(value, ty)
                } else {
                    value
                };
                ArgumentValue::Value(value)
            };
            args.push(CallArgument {
                parameter: a.parameter.0,
                passing,
                value,
            });
        }
        let dest = returns
            .map(|r| {
                let dest = self.place(r.destination)?;
                let address = self.address(&dest)?;
                Ok::<_, PassError>((dest, address))
            })
            .transpose()?;
        self.failure.cleanup = entered_cleanup;
        for a in &args {
            if let ArgumentValue::Value(v) = a.value {
                self.consumed(v);
            }
        }
        if let Callee::Indirect(v) = lir_callee {
            self.consumed(v);
        }
        for &(slot, version) in &writes {
            self.versions[slot.index()] = Some(version);
        }
        let result = fn_abi.result.as_ref().map(|r| {
            let ty = self.intern(r.layout);
            self.fresh(ty)
        });
        let capture_ends: Vec<_> = capture_loans
            .iter()
            .copied()
            .map(|loan| Action::EndLoan { loan })
            .collect();
        self.failure
            .before
            .splice(0..0, capture_ends.iter().cloned());
        let failure = self.fail(None)?;
        let anchor = self.anchor_here();
        let arguments = PoolRange::push(&mut self.out.pools.call_arguments, args);
        let call = CallId(self.out.tables.calls.len() as u32);
        self.out.tables.calls.push(CallSite {
            forward_caller: match callee {
                tir::Callee::Definition { .. } => true,
                tir::Callee::Indirect { forward_caller, .. } => *forward_caller,
            },
            callee: lir_callee,
            operation_arguments,
            abi,
            arguments,
            result,
            destination: fn_abi.destination.then_some(DestinationClass::Inferred),
            declared: CALL_EFFECTS,
            anchor,
        });
        let (Some(tir::CallReturn { next, .. }), Some((dest, dest_address))) = (returns, dest)
        else {
            // A diverging call has no result and no normal edge (LIR schema
            // revision 19); its failure edge is the frame's only way on.
            self.terminate(Terminator::Call {
                call,
                normal: None,
                failure,
            });
            return Ok(());
        };
        let after = self.block(BlockKind::Body);
        let normal = self.edge(after, Vec::new());
        self.terminate(Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        });
        self.current = after;
        self.failure.before.drain(0..capture_ends.len());
        for loan in capture_loans {
            self.push(Inst::EndLoan {
                loan,
                if_active: false,
            });
            self.state.loans[loan.index()] = Tri::No;
        }

        let value = match result {
            Some(v) => v,
            None => {
                let constant = self.constant(dest.ty, ConstantValue::Unit);
                let v = self.fresh(dest.ty);
                self.push(Inst::Const {
                    result: v,
                    constant,
                });
                v
            }
        };
        if result.is_some() && self.holds(value) {
            self.failure.pending.push(value);
        }
        if let Some(release) = release {
            // Leave the taken callee's cleanup only for this drop's failure.
            self.failure.before.drain(0..2);
            for action in &release {
                match self.action(action)? {
                    Step2::Inline(inst) => self.push(inst),
                    Step2::Drop(drop) => {
                        drop.apply(self);
                        let failure = self.fail(None)?;
                        self.push(Inst::Drop {
                            address: drop.address,
                            if_live: drop.if_live,
                            anchor: drop.anchor,
                            failure,
                        });
                    }
                    Step2::Skip => {}
                }
            }
        }
        self.store(&dest, dest_address, value)?;
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// The element count of a flat list part into `destination`.
    pub(super) fn flat_length(
        &mut self,
        p: &PlaceRef,
        destination: tir::PlaceId,
        next: tir::BlockId,
    ) -> R<()> {
        if flat_sequence(self.layouts, self.layout_of(p.ty)).is_none() {
            return malformed("the length of a flat part that is no sequence");
        }
        let address = self.address(p)?;
        let dest = self.place(destination)?;
        let dest_address = self.address(&dest)?;
        let result = self.value(dest.ty, Holding::Plain);
        self.push(Inst::FlatLength { result, address });
        self.store(&dest, dest_address, result)?;
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// The C convention of `path` when it is an extern (PROG-4), which is
    /// monomorphic.
    pub(super) fn foreign_abi(&mut self, path: &DefPath) -> R<Option<FnAbiId>> {
        let Some(signature) = self.program.signatures.get(path).cloned() else {
            return Ok(None);
        };
        let SignatureKind::Function(function) = &signature.kind else {
            return Ok(None);
        };
        if function.foreign.is_none() {
            return Ok(None);
        }
        let mut actuals = crate::instance::Actuals::none();
        let callable = crate::instance::instantiate_function(
            &signature,
            function,
            hirsel_types::Modes::global(),
            &mut actuals,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        self.layouts
            .fn_abi(&actuals.store, callable, CallableShape::Foreign)
            .map(Some)
            .map_err(|e| PassError::Layout(path.clone(), e))
    }

    pub(super) fn operation_abi(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> R<(FnAbiId, Vec<hirsel_lir::operation::OperationArgument>)> {
        let mut callee = crate::instance::callee_actuals(
            self.program.binders.get(path).copied(),
            &self.def.types,
            arguments,
            self.actuals,
            false,
            path,
        )?;
        let signature = self
            .program
            .signatures
            .get(path)
            .ok_or_else(|| InstanceError::MissingSignature(path.clone()))?
            .clone();
        let SignatureKind::Function(function) = &signature.kind else {
            return malformed(format!("{path:?} is called but is not a function"));
        };
        let callable = crate::instance::instantiate_function(
            &signature,
            function,
            hirsel_types::Modes::global(),
            &mut callee,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let arguments =
            hirsel_layout::operation::collect_arguments(&callee.store, &callee.arguments)
                .map_err(|e| PassError::Layout(path.clone(), e))?;
        let abi = self
            .layouts
            .fn_abi(&callee.store, callable, CallableShape::Definition)
            .map_err(|e| PassError::Layout(path.clone(), e))?;
        let abi = if signature.metadata.track_caller {
            self.layouts
                .with_caller_site(abi)
                .map_err(|e| PassError::Layout(path.clone(), e))?
        } else {
            abi
        };
        Ok((abi, arguments))
    }
}

/// The buffer and maximum parameters of an Io read that appends up to its
/// maximum bytes to a List (MEM-6).
fn read_room(path: &DefPath) -> Option<(u32, u32)> {
    if core_operation(path, "stdin", "read") {
        Some((1, 2))
    } else if core_operation(path, "file", "read")
        || core_operation(path, "net", "read")
        || core_operation(path, "udp", "recv_from")
    {
        Some((2, 3))
    } else {
        None
    }
}
