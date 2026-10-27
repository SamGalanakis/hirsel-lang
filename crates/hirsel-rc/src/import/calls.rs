use super::*;
use crate::PassError;

impl Importer<'_> {
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
                // A constructor-tail instance returns its root's payload.
                let v = match self.holes {
                    Some(_) => self.fill_return(v)?,
                    None => v,
                };
                let receipt = if self
                    .out
                    .tables
                    .receipt_entry
                    .as_ref()
                    .and_then(|e| e.result)
                    .is_some()
                {
                    let slot = self.receipt_slot(v).ok_or_else(|| {
                        PassError::Unsupported(
                            "receipt return has no paired metadata snapshot".into(),
                        )
                    })?;
                    let metadata = self.whole(slot);
                    let address = self.address(&metadata)?;
                    Some(self.operand_at(UseKind::Take, &metadata, address, None)?)
                } else {
                    None
                };
                let actions = self.exit_actions(self.chain_actions(*cleanup)?);
                self.failure = FailureContext {
                    cleanup: None,
                    before: Vec::new(),
                    pending: [Some(v).filter(|&v| self.holds(v)), receipt]
                        .into_iter()
                        .flatten()
                        .collect(),
                };
                self.run_normal(&actions, &[])?;
                if let Some(receipt) = receipt {
                    let slot = self
                        .out
                        .tables
                        .receipt_entry
                        .as_ref()
                        .and_then(|e| e.result)
                        .unwrap();
                    let metadata = self.whole(slot);
                    let address = self.address(&metadata)?;
                    self.store(&metadata, address, receipt)?;
                }
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
                self.invocation_live = false;
                self.invocation_return = returns.map(|r| r.next);
                self.failure = FailureContext {
                    cleanup: Some(*on_failure),
                    before: Vec::new(),
                    pending: Vec::new(),
                };
                let adapter = matches!(
                    callee,
                    tir::Callee::Indirect {
                        forward_caller: true,
                        ..
                    }
                );
                let leaf = matches!(callee, tir::Callee::Definition { path, .. }
                    if !hirsel_core_contract::depth::adds_activation(path));
                if !adapter && !leaf && self.plan.is_none_or(|p| p.site(t).is_none()) {
                    // The arguments have been evaluated into TIR temporaries.
                    // Until admission all their storage belongs to this caller;
                    // the entered-callee cleanup deliberately excludes takes.
                    self.failure.cleanup = None;
                    let failure = self.fail(Some("LANG-10".into()))?;
                    self.push(Inst::FrameEnter { failure });
                    self.invocation_live = true;
                    self.failure.cleanup = Some(*on_failure);
                }
                let key_address = self.key_preflight(callee, arguments)?;
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
                self.call(callee, arguments, *returns, key_address)?;
            }
            T::Match { table, from } => self.match_from(*table, *from)?,
        }
        Ok(())
    }

    fn key_preflight(
        &mut self,
        callee: &tir::Callee,
        arguments: &[tir::CallArgument],
    ) -> R<Option<(tir::PlaceId, AddressId)>> {
        let tir::Callee::Definition { path, .. } = callee else {
            return Ok(None);
        };
        if !self.program.operations.contains_key(path)
            || !["map.insert", "set.insert", "map.get", "set.contains"]
                .iter()
                .any(|name| path.is_core(name))
        {
            return Ok(None);
        }
        let key = arguments
            .iter()
            .find(|a| a.parameter.0 == 1)
            .ok_or_else(|| PassError::Malformed("keyed primitive lacks key actual".into()))?;
        let entered = self.failure.cleanup.take();
        let borrowed = tir::Operand {
            place: key.value.place,
            // A take actual is still caller-owned here. `compared` uses Copy
            // to open a read-only synthetic loan without copying or taking it.
            usage: if key.value.usage == UseKind::Take {
                UseKind::Copy
            } else {
                key.value.usage
            },
        };
        let mut loans = vec![];
        let mut owned = vec![];
        // Evaluate dynamic indices once. A second extraction can name a new
        // SSA index even when it reads the same unchanged source local.
        let address = if self.joined_view(&borrowed).is_some() {
            None
        } else {
            let place = self.place(borrowed.place)?;
            Some(self.address(&place)?)
        };
        let value = self.compared_at(&borrowed, address, &mut loans, &mut owned)?;
        let layout = self.layout_of(self.value_type(value));
        self.structural_access(self.body.places[key.value.place.0 as usize].ty, layout)?;
        let scalar = self.types.intern(hirsel_types::Type::with_kind(
            hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::U64),
        ));
        let ty = self.lir_type(scalar)?;
        let result = self.fresh(ty);
        self.reading(loans, owned, |this| {
            let failure = this.fail(Some("MEM-3".into()))?;
            this.push(Inst::Hash {
                result,
                value,
                layout,
                failure,
            });
            Ok(())
        })?;
        self.failure.cleanup = entered;
        Ok(address.map(|address| (key.value.place, address)))
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
        key_address: Option<(tir::PlaceId, AddressId)>,
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
        if let Some(source) = self.slices.sources.get(&self.call_block).copied() {
            let ret = returns.ok_or_else(|| PassError::Malformed("slice continuation".into()))?;
            return self.slice_view(arguments, ret, source);
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
            let saved = self.save();
            let views = self.views.clone();
            let facts = self.facts.clone();
            self.current = done;
            self.removed_call(returns)?;
            self.restore(saved);
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
                } else if self.instances.has_function(path)
                    || self
                        .program
                        .instance_bodies
                        .declarations
                        .contains(&DefId::intern(path))
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
                        return Err(
                            InstanceError::MissingInstance(self.def.path.path().clone()).into()
                        );
                    };
                    let digest = self.instances.view(instance).key.digest();
                    let code = self.symbol(
                        SymbolKind::Function,
                        &self.def.path.path().clone(),
                        Some(digest),
                    );
                    Some((code, self.instances.abis[instance.index()]))
                };
                let abi = match bound {
                    Some((_, abi)) => abi,
                    None => {
                        let ty = self.ty(self.body.places[value.place.0 as usize].ty)?;
                        self.layouts
                            .fn_abi(self.types, ty, CallableShape::Closure)
                            .map_err(|e| PassError::Layout(self.def.path.path().clone(), e))?
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
                let value = if let Some((place, address)) = key_address
                    && a.parameter.0 == 1
                    && place == a.value.place
                {
                    let place = self.place(place)?;
                    self.operand_at(a.value.usage, &place, address, place.loan)?
                } else {
                    self.operand(&a.value)?
                };
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
        let mut receipts = if let Some(contract) = &fn_abi.receipts {
            if !contract.captures.is_empty()
                || contract.inventory.iter().any(|grant| {
                    !matches!(
                        grant,
                        hirsel_lir::sealed_payload_source::ReceiptGrant::Parameter(_)
                    )
                })
            {
                return unsupported("receipt calls with captures or non-parameter inventory");
            }
            let mut carriers = Vec::new();
            for parameter in &contract.parameters {
                let argument = arguments
                    .iter()
                    .find(|a| a.parameter.0 == parameter.source)
                    .ok_or_else(|| PassError::Malformed("receipt call source parameter".into()))?;
                let place = &self.body.places[argument.value.place.0 as usize];
                if !place.projection.is_empty() {
                    return unsupported(
                        "receipt call projected argument requires complete payload transport",
                    );
                }
                let value = args
                    .iter()
                    .find(|a| a.parameter == parameter.source)
                    .and_then(|a| match a.value {
                        ArgumentValue::Value(value) => Some(value),
                        ArgumentValue::Address { address, .. } => {
                            let AddressBase::Slot(slot) = self.out.addresses[address.index()].base
                            else {
                                return None;
                            };
                            self.versions[slot.index()]
                        }
                    })
                    .ok_or_else(|| PassError::Malformed("receipt ordinary argument".into()))?;
                let slot = self.receipt_slot(value).ok_or_else(|| {
                    PassError::Unsupported(
                        "receipt call argument has no paired metadata snapshot".into(),
                    )
                })?;
                let metadata = self.whole(slot);
                let address = self.address(&metadata)?;
                let carrier = match parameter.mode {
                    hirsel_lir::sealed_payload_source::ReceiptMode::Borrow => {
                        let loan = self.synthetic_loan(address);
                        let receipt = self.value(metadata.ty, Holding::Borrowed(loan));
                        let reaching = self.reaching(slot)?;
                        self.push(Inst::Read {
                            result: receipt,
                            address,
                            reaching,
                            mode: ReadMode::Borrow,
                        });
                        capture_loans.push(loan);
                        hirsel_lir::receipt_transport::ReceiptArgument::Borrow {
                            parameter: parameter.source,
                            receipt,
                        }
                    }
                    hirsel_lir::sealed_payload_source::ReceiptMode::Take => {
                        let receipt = self.operand_at(UseKind::Take, &metadata, address, None)?;
                        hirsel_lir::receipt_transport::ReceiptArgument::Take {
                            parameter: parameter.source,
                            receipt,
                        }
                    }
                    hirsel_lir::sealed_payload_source::ReceiptMode::MutSlot => {
                        let version = self.value(metadata.ty, Holding::Plain);
                        writes.push((slot, version));
                        hirsel_lir::receipt_transport::ReceiptArgument::Mut {
                            parameter: parameter.source,
                            address,
                            version,
                        }
                    }
                };
                carriers.push(carrier);
            }
            Some(hirsel_lir::receipt_transport::ReceiptCall {
                contract: contract
                    .content()
                    .map_err(|e| PassError::Malformed(e.into()))?,
                arguments: carriers,
                result: None,
            })
        } else {
            None
        };
        let dest = returns
            .map(|r| {
                let dest = self.place(r.destination)?;
                let address = self.address(&dest)?;
                Ok::<_, PassError>((dest, address))
            })
            .transpose()?;
        let capture_ends: Vec<_> = capture_loans
            .iter()
            .copied()
            .map(|loan| Action::EndLoan { loan })
            .collect();
        self.failure
            .before
            .splice(0..0, capture_ends.iter().cloned());
        let pretransfer_failure =
            if matches!(lir_callee, Callee::Indirect(_) | Callee::Bound { .. }) {
                Some(self.fail(None)?)
            } else {
                None
            };
        self.failure.cleanup = entered_cleanup;
        for a in &args {
            if let ArgumentValue::Value(v) = a.value {
                self.consumed(v);
            }
        }
        if let Callee::Indirect(v) = lir_callee {
            self.consumed(v);
        }
        if let Some(receipts) = &receipts {
            for receipt in receipts.inputs() {
                self.consumed(receipt);
            }
        }
        for &(slot, version) in &writes {
            self.versions[slot.index()] = Some(version);
        }
        let result = fn_abi.result.as_ref().map(|r| {
            let ty = self.intern(r.layout);
            self.fresh(ty)
        });
        let receipt_result = if fn_abi.receipts.as_ref().is_some_and(|c| c.result.is_some()) {
            let symbol = match lir_callee {
                Callee::Direct(id) | Callee::Bound { code: id, .. } => {
                    &self.out.tables.symbols[id.index()]
                }
                _ => return unsupported("receipt result requires a selected source callee"),
            };
            let source = (0..self.instances.len())
                .filter_map(|index| self.instances.receipt_source(InstanceId(index as u32)))
                .find(|source| {
                    source.original.definition == symbol.definition
                        && Some(source.instance) == symbol.instance
                })
                .ok_or_else(|| {
                    PassError::Unsupported("receipt call result lacks its original profile".into())
                })?;
            let profile = source
                .profile(hirsel_lir::sealed_payload_source::ReceiptPort::Result)
                .ok_or_else(|| {
                    PassError::Malformed("receipt call result original profile".into())
                })?;
            let layout = self.layouts.payload_receipt(&profile.root.ty);
            let ty = self.intern(layout);
            let value = self.fresh(ty);
            receipts.as_mut().unwrap().result = Some(value);
            Some(value)
        } else {
            None
        };
        let failure = self.fail(None)?;
        let anchor = self.anchor_here();
        let arguments = PoolRange::push(&mut self.out.pools.call_arguments, args);
        let call = CallId(self.out.tables.calls.len() as u32);
        self.out.tables.calls.push(CallSite {
            receipts,
            pretransfer_failure,
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
        if let Some(receipt) = receipt_result {
            self.failure.pending.push(receipt);
            let ty = self.value_type(receipt);
            let slot = self.new_slot(ty, SlotKind::Local, ScopeId(0));
            self.push(Inst::StorageLive { slot });
            self.state.live(slot);
            let metadata = self.whole(slot);
            let address = self.address(&metadata)?;
            self.store(&metadata, address, receipt)?;
            self.receipt_values.insert(value, slot);
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

    /// Follow whole-value moves and ordinary shared copies to their snapshot.
    /// Projections, deep copies, new payloads and unproved joins need their own
    /// receipt producer. C recovers the final pairing independently.
    pub(super) fn receipt_slot(&self, mut value: ValueId) -> Option<SlotId> {
        let whole = |mut address: AddressId| {
            for _ in 0..=self.out.addresses.len() {
                let row = &self.out.addresses[address.index()];
                if row.projection.len != 0 {
                    return false;
                }
                match row.base {
                    AddressBase::Slot(_) => return true,
                    AddressBase::Address(parent) => address = parent,
                    AddressBase::Value(_) => return false,
                }
            }
            false
        };
        let mut aliases = vec![None; self.out.values.len()];
        for instruction in self.out.blocks.iter().flat_map(|block| &block.insts) {
            let (result, source) = match *instruction {
                Inst::Weaken { result, value } => (result, value),
                Inst::Read {
                    result,
                    reaching,
                    address,
                    ..
                }
                | Inst::Copy {
                    result,
                    reaching,
                    address,
                    ..
                } if whole(address) => (result, reaching),
                Inst::Store {
                    version,
                    value,
                    address,
                }
                | Inst::Replace {
                    version,
                    value,
                    address,
                    ..
                } if whole(address) => (version, value),
                _ => continue,
            };
            aliases[result.index()] = Some(source);
        }
        for call in &self.out.tables.calls {
            for (address, version) in call.writebacks(&self.out.pools) {
                if let AddressBase::Slot(slot) = self.out.addresses[address.index()].base
                    && let SlotKind::Parameter(source) = self.out.slots[slot.index()].kind
                    && let Some(&parameter) = self.out.parameters.get(source as usize)
                {
                    aliases[version.index()] = Some(parameter);
                }
            }
        }
        for block in &self.out.blocks {
            for &(slot, version) in &block.phis {
                if let SlotKind::Parameter(source) = self.out.slots[slot.index()].kind
                    && let Some(&parameter) = self.out.parameters.get(source as usize)
                {
                    aliases[version.index()] = Some(parameter);
                }
            }
        }
        for _ in 0..=aliases.len() {
            if let Some(&slot) = self.receipt_values.get(&value) {
                return Some(slot);
            }
            if let Some(ordinal) = self.out.parameters.iter().position(|v| *v == value) {
                let source = self.own_abi().parameters.get(ordinal)?.source;
                return self
                    .out
                    .tables
                    .receipt_entry
                    .as_ref()?
                    .parameters
                    .iter()
                    .find(|p| p.source == source)
                    .map(|p| p.slot);
            }
            value = aliases[value.index()]?;
        }
        None
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
        let actuals = crate::instance::Actuals::none();
        let callable = crate::instance::instantiate_function(
            &self.program.types,
            &self.program.memo,
            function,
            hirsel_types::Modes::global(),
            &actuals,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        self.layouts
            .fn_abi(self.types, callable, CallableShape::Foreign)
            .map(Some)
            .map_err(|e| PassError::Layout(path.clone(), e))
    }

    pub(super) fn operation_abi(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> R<(FnAbiId, Vec<hirsel_lir::operation::OperationArgument>)> {
        let callee = crate::instance::callee_actuals(
            self.types,
            &self.program.memo,
            self.program.binders.get(path).copied(),
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
            &self.program.types,
            &self.program.memo,
            function,
            hirsel_types::Modes::global(),
            &callee,
        )
        .map_err(|e| InstanceError::Substitution(path.clone(), e))?;
        let arguments = hirsel_layout::operation::collect_arguments(self.types, &callee.arguments)
            .map_err(|e| PassError::Layout(path.clone(), e))?;
        let abi = self
            .layouts
            .fn_abi(self.types, callable, CallableShape::Definition)
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
        || core_operation(path, "read_half", "read")
        || core_operation(path, "udp", "recv_from")
    {
        Some((2, 3))
    } else {
        None
    }
}
