//! Resumable continuation alternates (r6 S7), as `hirsel-cranelift`'s `resume`
//! emits them: one alternate per resume state and a dispatcher, each with the
//! body's signature plus a trailing frame pointer. Declared parameters, region
//! records and storage live at any boundary live in the frame; state 0 stores
//! the parameters once, and each boundary materializes its state's live values and
//! state number first, and state k reloads them and reissues its boundary call.
//! Alternates keep sequence lengths in memory: no register state survives a
//! suspension.
use super::*;
use hirsel_lir::continuation::{
    ContinuationPlan, ContinuationPlans, ExclusiveSlot, NestedFrame, Representation, ResumeAction,
    ResumeState, CleanupResumeState, LiveOwnedSlot, resume_name, resume_site_hint,
};
use hirsel_lir::runtime_abi::STATUS_SUSPENDED;

pub(super) struct Resume<'a, 'm> {
    pub(super) plan: &'a ContinuationPlan,
    pub(super) state: u32,
    pub(super) frame: Value<'m>,
    /// The boundary call this alternate starts at: its protocol already ran.
    pub(super) resuming: Option<CallId>,
}

/// The body's signature with the frame pointer appended.
pub(super) fn resume_type<'m>(module: &'m Module<'m>, signature: &Signature<'m, '_>) -> Type<'m> {
    let mut types = signature.types.clone();
    types.push(module.ptr_type());
    module.function_type(signature.returns, &types)
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    fn frame_place(&self, offset: u64, layout: LayoutId) -> Emitted<Place<'m>> {
        let frame = self.resume.as_ref().expect("resume frame").frame;
        let align = self.layout(layout)?.align;
        Ok(Place {
            columns: None,
            ptr: self.builder.offset(frame, self.const_u64(offset)),
            layout,
            align: align.min(low_bit(offset)).min(16),
            block: None,
        })
    }

    /// Storage live at a boundary and every region record are frame-resident
    /// in every state.
    pub(super) fn resume_storage(&mut self) -> Emitted {
        let Some(resume) = self.resume.as_ref() else {
            return Ok(());
        };
        let (plan, frame) = (resume.plan, resume.frame);
        for storage in &plan.storage {
            if let SlotKind::Capture(_) = self.unit.slots[storage.slot.index()].kind {
                continue;
            }
            let align = self.layout(storage.layout)?.align;
            self.slots[storage.slot.index()] = Some(Place {
                columns: None,
                ptr: self.prologue.offset(frame, self.const_u64(storage.offset)),
                layout: storage.layout,
                align: align.min(low_bit(storage.offset)).min(16),
                block: None,
            });
        }
        if plan.records.len() != self.regions.len() {
            return self.invalid("a resumable plan without every region record");
        }
        for record in &plan.records {
            self.regions[record.region.index()] =
                self.prologue.offset(frame, self.const_u64(record.offset));
        }
        Ok(())
    }

    /// Declared parameters, materialized once: state 0 stores each incoming
    /// value (an address parameter's bytes) in its frame record, and every
    /// state binds the parameter from that record, never from a re-entry's
    /// arguments, which the caller passes again unchanged. An exclusive
    /// parameter's record holds the caller slot's address, and its slot is that
    /// place: every access reaches the caller's canonical bytes.
    pub(super) fn resume_parameters(&mut self) -> Emitted {
        let (plan, state) = {
            let resume = self.resume.as_ref().unwrap();
            (resume.plan, resume.state)
        };
        let abi = self.signature.abi;
        if plan.parameters.len() != abi.parameters.len() || !self.signature.roots.is_empty() {
            return self.invalid("a resumable plan off its body's parameters");
        }
        self.builder.position(self.prologue_block);
        for (i, (parameter, record)) in abi.parameters.iter().zip(&plan.parameters).enumerate() {
            if parameter.passing == ParameterPassing::ExclusiveAddress {
                let (Some(slot), ParameterPassing::ExclusiveAddress) =
                    (record.slot, record.passing)
                else {
                    return self
                        .invalid("an exclusive parameter without its slot's address record");
                };
                let frame = self.resume.as_ref().unwrap().frame;
                let at = self.builder.offset(frame, self.const_u64(record.offset));
                if state == 0 {
                    let incoming = self
                        .module
                        .parameter(self.function, self.signature.first[i]);
                    self.builder.store(incoming, at, 8);
                }
                let ptr = self.builder.load(self.module.ptr_type(), at, 8);
                self.slots[slot.index()] = Some(Place {
                    columns: None,
                    ptr,
                    layout: record.layout,
                    align: self.layout(record.layout)?.align,
                    block: None,
                });
                continue;
            }
            let shape = self.layout_shape(record.layout)?;
            let place = self.frame_place(record.offset, record.layout)?;
            if state == 0 {
                let first = self.signature.first[i];
                let value = match parameter.passing {
                    ParameterPassing::ByValue => {
                        let args: Vec<_> = (0..parameter.slots.len() as u32)
                            .map(|k| self.module.parameter(self.function, first + k))
                            .collect();
                        self.assemble(record.layout, &args)?
                    }
                    ParameterPassing::ReadOnlyAddress | ParameterPassing::TransferAddress => {
                        let incoming = Place {
                            columns: None,
                            ptr: self.module.parameter(self.function, first),
                            layout: record.layout,
                            align: self.layout(record.layout)?.align,
                            block: None,
                        };
                        self.load(shape, incoming)
                    }
                    _ => return self.invalid("a resumable parameter the frame cannot hold"),
                };
                self.store(shape, value, place);
            }
            // A value the state reloads is defined by its reload.
            let reloaded = plan.states.iter().any(|s|s.target.state==state && s.live.iter().any(|l|l.value==record.value))
                || plan.cleanups.iter().any(|s|s.state==state && s.live.iter().any(|l|l.value==record.value));
            match record.slot {
                Some(slot) => self.slots[slot.index()] = Some(place),
                None if reloaded => {}
                None => {
                    let value = self.load(shape, place);
                    self.define(record.value, value)?;
                }
            }
        }
        self.bind_captures()?;
        self.bind_receipt_entry()
    }

    /// Store a state's live values and number before its boundary call.
    fn materialize(&mut self, state: &ResumeState) -> Emitted {
        self.materialize_live(&state.live,state.target.state)
    }
    fn materialize_live(&mut self,live:&[LiveOwnedSlot],number:u32)->Emitted {
        for live in live {
            if self.versions.contains(&live.value) {
                continue;
            }
            let shape = self.value_shape(live.value)?;
            let value = self.get(live.value)?;
            let place = self.frame_place(live.offset, live.layout)?;
            self.store(shape, value, place);
        }
        let frame = self.resume.as_ref().unwrap().frame;
        let number = self
            .module
            .const_int(self.module.int_type(32), u128::from(number));
        self.builder.store(number, frame, 8);
        Ok(())
    }

    /// Define a state's live values from the frame; bytes get fresh copies.
    /// A value a skipped block's phi defined is redefined by its reload.
    fn reload(&mut self, state: &ResumeState) -> Emitted {
        self.reload_live(&state.live)
    }
    fn reload_live(&mut self,live:&[LiveOwnedSlot])->Emitted {
        for live in live {
            if self.versions.contains(&live.value) {
                continue;
            }
            let shape = self.value_shape(live.value)?;
            let place = self.frame_place(live.offset, live.layout)?;
            let value = match shape {
                Shape::Bytes { .. } => {
                    let copy = self.stack(live.layout)?;
                    let loaded = self.load(shape, place);
                    self.store(shape, loaded, copy);
                    self.load(shape, copy)
                }
                _ => self.load(shape, place),
            };
            self.values[live.value.index()] = Some(value);
        }
        Ok(())
    }

    fn return_suspended(&mut self) -> Emitted {
        self.return_status(u128::from(STATUS_SUSPENDED), None)
    }

    /// The proof that nested boundary `call` passes callee parameter `source`
    /// as a slot of this frame.
    pub(super) fn resume_exclusive(&self, call: CallId, source: u32) -> Option<&'a ExclusiveSlot> {
        let plan = self.resume.as_ref()?.plan;
        plan.states
            .iter()
            .find(|s| s.boundary.call == call)?
            .exclusive
            .as_ref()
            .filter(|e| e.call == call && e.parameter == source)
    }

    /// The stable address of a proved exclusive slot: its offset in this
    /// frame's allocation, never a temporary.
    pub(super) fn exclusive_slot_address(&self, proof: &ExclusiveSlot) -> Emitted<Value<'m>> {
        let frame = self.resume.as_ref().expect("resume frame").frame;
        let bound = self.slots.get(proof.slot.index()).copied().flatten();
        if !bound.is_some_and(|p| p.columns.is_none() && p.layout == proof.layout)
            || !self
                .resume
                .as_ref()
                .unwrap()
                .plan
                .storage
                .iter()
                .any(|s| s.slot == proof.slot && s.offset == proof.offset)
        {
            return self.invalid("an exclusive slot outside this frame's storage");
        }
        Ok(self.builder.offset(frame, self.const_u64(proof.offset)))
    }

    /// Whether `call` is a boundary of this body's resumable plan.
    pub(crate) fn resume_boundary_call(&self, call: CallId) -> bool {
        self.resume
            .as_ref()
            .is_some_and(|r| r.plan.states.iter().any(|s| s.boundary.call == call))
    }

    /// The boundary protocol before `call`; for a resumable callee, its frame.
    pub(super) fn resume_boundary(
        &mut self,
        call: CallId,
    ) -> Emitted<Option<(Value<'m>, NestedFrame)>> {
        let Some(resume) = self.resume.as_mut() else {
            return Ok(None);
        };
        let plan = resume.plan;
        let Some(state) = plan.states.iter().find(|s| s.boundary.call == call) else {
            return Ok(None);
        };
        let resuming = resume.resuming == Some(call);
        if resuming {
            resume.resuming = None;
        }
        let frame = resume.frame;
        let callee = match state.action {
            ResumeAction::ResumeNested(at, frame) if at == call && state.callee == Some(frame) => {
                Some(frame)
            }
            ResumeAction::RetryUnenteredCall(at)
                if at == call && state.callee.is_none() && state.runtime.is_none() =>
            {
                None
            }
            ResumeAction::ResumeRuntime(at)
                if at == call && state.callee.is_none() && state.runtime.is_some() =>
            {
                None
            }
            ResumeAction::AfterEnteredCall(at, edge)
                if at == call
                    && edge == state.target.edge
                    && state.callee.is_none()
                    && state.tail.is_some() =>
            {
                if !resuming {
                    self.materialize(state)?;
                }
                return Ok(None);
            }
            _ => return self.invalid("unsupported or inconsistent resume action"),
        };
        if let Some(callee) = callee {
            let nested = self.builder.offset(frame, self.const_u64(callee.offset));
            if !resuming {
                self.materialize(state)?;
                self.builder
                    .store(self.module.const_zero(self.module.int_type(32)), nested, 16);
            }
            return Ok(Some((nested, callee)));
        }
        if !resuming {
            self.materialize(state)?;
            let point = self.runtime(RuntimeEntry::ResumePoint)?;
            let hint = resume_site_hint(self.unit, state);
            let number = self.module.const_int(
                self.module.int_type(32),
                u128::from(state.target.state | hint),
            );
            let decided = self.builder.call(point, &[self.context, frame, number]);
            let suspended = self.builder.icmp(
                IntPredicate::Ne,
                decided,
                self.module.const_zero(self.module.int_type(8)),
            );
            let (cold, proceed) = (
                self.module.append_block(self.function),
                self.module.append_block(self.function),
            );
            self.builder.cold_cond_br(suspended, cold, proceed);
            self.builder.position(cold);
            self.return_suspended()?;
            self.builder.position(proceed);
        }
        Ok(None)
    }

    pub(super) fn resume_cleanup_bind(&mut self,block:BlockId,instruction:usize)->Emitted {
        let Some(resume)=self.resume.as_ref() else {return Ok(())};
        let Some(state)=resume.plan.cleanups.iter().find(|s|s.block==block&&s.instruction==instruction as u32) else {return Ok(())};
        let (state,bytes,frame)=(state.clone(),resume.plan.cost.frame_bytes,resume.frame);
        self.materialize_live(&state.live,state.state)?;
        let entry=self.runtime(RuntimeEntry::ResumeTail)?;
        let number=self.module.const_int(self.module.int_type(32),state.state.into());
        let tag = if matches!(self.unit.types[self.unit.values[state.value.index()].ty.index()].repr, ValueRepr::Block) {
            if matches!(self.layout(state.layout)?.kind, hirsel_lir::layout_types::LayoutKind::Shared { .. }) { 48 } else { 49 }
        } else { 50 };
        let kind=self.module.const_int(self.module.int_type(32),tag);
        let bytes=self.module.const_int(self.module.int_type(32),bytes.into());
        let (anchor,site)=self.failure_sites(self.cleanup_code(state.anchor)?);
        self.builder.call(entry,&[self.context,frame,number,kind,bytes,anchor,site]);
        Ok(())
    }

    /// A resumable callee's dispatcher, called with its nested frame.
    pub(super) fn resume_callee(
        &mut self,
        symbol: SymbolId,
        callee: NestedFrame,
        signature: &Signature<'m, '_>,
    ) -> Emitted<Function<'m>> {
        let Some(symbol) = self.unit.tables.symbols.get(symbol.index()) else {
            return self.invalid("a callee outside the symbol table");
        };
        let name = if self.inputs.alternate {
            hirsel_lir::ahead::alternate_resume_name(&symbol.definition, callee.body)
        } else {
            resume_name(&symbol.definition, callee.body, None)
        };
        self.manifested
            .imports
            .entry(name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(symbol.definition.clone()),
                abi: None,
            });
        self.module
            .function(&name, resume_type(self.module, signature))
            .map_err(|what| CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })
    }

    pub(super) fn runtime_result(
        &self,
        call: CallId,
        layout: LayoutId,
    ) -> Emitted<Option<Place<'m>>> {
        let Some(state) = self
            .resume
            .as_ref()
            .and_then(|r| r.plan.states.iter().find(|s| s.boundary.call == call))
        else {
            return Ok(None);
        };
        let Some(runtime) = &state.runtime else {
            return Ok(None);
        };
        let [slot] = runtime.results.as_slice() else {
            return self.invalid("entered intrinsic result cell count");
        };
        if slot.layout != layout
            || !matches!(slot.source,hirsel_lir::continuation::RuntimeResultSource::CallResult(value) if Some(value)==self.unit.tables.calls[call.index()].result)
        {
            return self.invalid("entered intrinsic result cell differs from ABI");
        }
        self.frame_place(slot.offset, slot.layout).map(Some)
    }

    pub(super) fn bind_runtime(
        &mut self,
        call: CallId,
        name: Option<&str>,
        codes: (Value<'m>, Value<'m>),
    ) -> Emitted {
        let Some((plan, state, frame)) = self.resume.as_ref().and_then(|r| {
            r.plan
                .states
                .iter()
                .find(|s| s.boundary.call == call)
                .map(|s| (r.plan, s, r.frame))
        }) else {
            return Ok(());
        };
        let Some(runtime) = &state.runtime else {
            return Ok(());
        };
        if state.action != ResumeAction::ResumeRuntime(call)
            || name != Some(runtime.helper.as_str())
        {
            return self.invalid("entered runtime helper differs from its proof");
        }
        let point = self.runtime(RuntimeEntry::ResumeTail)?;
        let number = self
            .module
            .const_int(self.module.int_type(32), u128::from(state.target.state));
        let kind = self
            .module
            .const_int(self.module.int_type(32), u128::from(runtime.kind.tag()));
        let bytes = self
            .module
            .const_int(self.module.int_type(32), u128::from(plan.cost.frame_bytes));
        self.builder.call(
            point,
            &[self.context, frame, number, kind, bytes, codes.0, codes.1],
        );
        Ok(())
    }

    pub(super) fn resume_tail_bind(&mut self, call: CallId) -> Emitted {
        let Some(resume) = self.resume.as_ref() else {
            return Ok(());
        };
        let Some(state) = resume
            .plan
            .states
            .iter()
            .find(|s| s.boundary.call == call && s.tail.is_some())
        else {
            return Ok(());
        };
        let frame = resume.frame;
        let number = self
            .module
            .const_int(self.module.int_type(32), u128::from(state.target.state));
        let kind = self.module.const_int(
            self.module.int_type(32),
            state.tail.as_ref().unwrap().kind as u128,
        );
        let bytes = self.module.const_int(
            self.module.int_type(32),
            u128::from(resume.plan.cost.frame_bytes),
        );
        let site = &self.unit.tables.calls[call.index()];
        let code = self.anchor_code(site.anchor)?;
        let codes = self.caller_sites(code, site.forward_caller);
        let entry = self.runtime(RuntimeEntry::ResumeTail)?;
        self.builder.call(
            entry,
            &[self.context, frame, number, kind, bytes, codes.0, codes.1],
        );
        Ok(())
    }

    /// After a nested resumable call: a suspended callee suspends this frame,
    /// whose state the boundary already recorded.
    pub(super) fn resume_propagate(&mut self, status: Value<'m>) -> Emitted {
        let suspended = self.builder.icmp(
            IntPredicate::Eq,
            status,
            self.module
                .const_int(self.module.int_type(8), u128::from(STATUS_SUSPENDED)),
        );
        let (cold, next) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.cold_cond_br(suspended, cold, next);
        self.builder.position(cold);
        self.return_suspended()?;
        self.builder.position(next);
        Ok(())
    }

    /// One state alternate: state 0 enters the body; state k reloads its
    /// values and reissues its boundary call, emitting only what follows.
    pub(super) fn emit_resume(mut self) -> Emitted {
        let (plan, state) = {
            let resume = self.resume.as_ref().unwrap();
            (resume.plan, resume.state)
        };
        self.find_versions()?;
        for _ in &self.unit.blocks {
            self.blocks.push(self.module.append_block(self.function));
        }
        self.create_phis()?;
        self.allocate_slots()?;
        self.bind_parameters()?;
        self.ahead_init()?;
        let start = self.module.append_block(self.function);
        self.builder.position(start);
        let first = match state {
            0 => self.unit.entry,
            k => match plan.states.get(k as usize - 1) {
                Some(s) => s.boundary.block,
                None => match plan.cleanups.iter().find(|s|s.state==k) {
                    Some(s)=>s.block,None=>return self.invalid("a resume state outside its plan"),
                },
            },
        };
        let cleanup=plan.cleanups.iter().find(|s|s.state==state);
        let count = self.unit.blocks.len();
        let mut adjacent: Vec<Vec<usize>> = (0..count)
            .map(|b| self.successors(b))
            .collect::<Emitted<_>>()?;
        if state != 0 && cleanup.is_none() {
            let at = plan.states[state as usize - 1].boundary.block;
            adjacent[at.index()] = hirsel_lir::continuation::resumed_successors(self.unit, at)
                .iter()
                .map(|b| b.index())
                .collect();
        }
        let order = hirsel_graph::reverse_postorder(&adjacent, first.index());
        if state == 0 {
            self.builder.br(self.blocks[self.unit.entry.index()]);
        } else if let Some(s)=cleanup {
            self.reload_live(&s.live)?;
            self.emit_block_suffix(s.block,s.instruction as usize,true)?;
        } else {
            let s = &plan.states[state as usize - 1];
            let mut skipped = vec![true; count];
            for &b in &order {
                skipped[b] = b == first.index();
            }
            self.reload(s)?;
            let unit = self.unit;
            for (block, i) in
                hirsel_lir::continuation::resume_preparations(unit, plan, state, &skipped)
            {
                self.at = (block, i);
                let inst =
                    &self.pool(unit.blocks[block.index()].instructions, &unit.instructions)?[i];
                self.instruction(inst)?;
            }
            self.resume.as_mut().unwrap().resuming = Some(s.boundary.call);
            self.at = (first, 0);
            self.terminator(&unit.terminators[first.index()])?;
        }
        let mut emitted = vec![false; count];
        for b in order {
            if state != 0 && b == first.index() {
                continue;
            }
            emitted[b] = true;
            self.emit_block(BlockId(b as u32))?;
        }
        // Blocks before this state's boundary never run here.
        for (b, done) in emitted.into_iter().enumerate() {
            if !done {
                self.builder.position(self.blocks[b]);
                self.builder.unreachable();
            }
        }
        self.builder.position(self.prologue_block);
        self.builder.br(start);
        Ok(())
    }

    /// A resumable task body's ordinary entry: the runtime runs its dispatcher
    /// over an owned frame, saving that frame at each suspension.
    pub(super) fn emit_resume_entry(self, plan: &ContinuationPlan) -> Emitted {
        let arity = self.signature.types.len() as u32;
        if arity != 4 || self.signature.value.is_some() || !self.signature.roots.is_empty() {
            return self.invalid("a resumable task body off the task entry signature");
        }
        let header = &self.unit.header;
        let name = resume_name(&header.definition, header.instance, None);
        self.manifested
            .imports
            .entry(name.clone())
            .or_insert_with(|| ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(header.definition.clone()),
                abi: None,
            });
        let dispatcher = self
            .module
            .function(&name, resume_type(self.module, &self.signature))
            .map_err(|what| CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        let mut this = self;
        let run = this.runtime(RuntimeEntry::ResumeRun)?;
        this.builder.position(this.prologue_block);
        let mut args: Vec<_> = (0..arity)
            .map(|i| this.module.parameter(this.function, i))
            .collect();
        args.push(dispatcher.value);
        args.push(
            this.module
                .const_int(this.module.int_type(32), u128::from(plan.cost.frame_bytes)),
        );
        let status = this.builder.call(run, &args);
        let returned = this.module.const_zero(this.signature.returns);
        this.builder.ret(
            this.signature
                .returned(&this.builder, returned, status, None),
        );
        Ok(())
    }
}

/// Every resumable plan's state alternates and dispatcher, declared into
/// `module`: the functions `lower` then emits, and each dispatcher's body.
pub(super) fn declare<'m, 'a>(
    module: &'m Module<'m>,
    inputs: &'a EmitInputs<'a>,
    units: &[&'a ValidatedUnit<Ready>],
    plans: &'a ContinuationPlans,
    manifested: &mut Manifested,
) -> Result<Vec<(&'a LirUnit<Ready>, &'a ContinuationPlan, u32, Function<'m>)>, CodegenError> {
    let mut states = Vec::new();
    for unit in units {
        let unit = unit.unit();
        let header = &unit.header;
        let Some(plan) = plans
            .get(header.instance)
            .filter(|p| p.representation == Representation::Resumable)
        else {
            continue;
        };
        let abi = fn_abi(inputs.layouts, header.abi, &header.definition)?;
        let signature = Signature::of(module, inputs.layouts, abi, &header.definition)?;
        let ty = resume_type(module, &signature);
        // Ahead code has only its state 0: a save continues in the ordinary
        // dispatcher, which reissues the boundary.
        if inputs.alternate {
            let name =
                hirsel_lir::ahead::alternate_resume_name(&header.definition, header.instance);
            if module.defines(&name) {
                return Err(CodegenError::DuplicateSymbol { name });
            }
            let function = module
                .function(&name, ty)
                .map_err(|what| CodegenError::Invalid {
                    definition: header.definition.clone(),
                    what,
                })?;
            module.add_attribute(function, AttributeSlot::Function, Attribute::NoUnwind);
            module.add_attribute(function, AttributeSlot::Function, Attribute::NoInline);
            module.add_attribute(function, AttributeSlot::Function, Attribute::NoRedZone);
            module.string_attribute(function, "disable-tail-calls", "true");
            module.string_attribute(function, "frame-pointer", "all");
            manifested.exports.insert(
                name.clone(),
                ObjectSymbol {
                    name,
                    kind: ObjectSymbolKind::Function,
                    definition: Some(header.definition.clone()),
                    abi: None,
                },
            );
            states.push((unit, plan, 0, function));
            continue;
        }
        let mut declared = Vec::new();
        for state in std::iter::once(None).chain((0..=(plan.states.len()+plan.cleanups.len()) as u32).map(Some)) {
            let name = resume_name(&header.definition, header.instance, state);
            if module.defines(&name) {
                return Err(CodegenError::DuplicateSymbol { name });
            }
            let function = module
                .function(&name, ty)
                .map_err(|what| CodegenError::Invalid {
                    definition: header.definition.clone(),
                    what,
                })?;
            module.add_attribute(function, AttributeSlot::Function, Attribute::NoUnwind);
            manifested.exports.insert(
                name.clone(),
                ObjectSymbol {
                    name,
                    kind: ObjectSymbolKind::Function,
                    definition: Some(header.definition.clone()),
                    abi: None,
                },
            );
            declared.push(function);
        }
        // The dispatcher enters the state its frame header names.
        let dispatcher = declared[0];
        let builder = Builder::new(module);
        let entry = module.append_block(dispatcher);
        builder.position(entry);
        let arity = signature.types.len() as u32 + 1;
        let params: Vec<_> = (0..arity)
            .map(|i| module.parameter(dispatcher, i))
            .collect();
        let frame = *params.last().unwrap();
        let number = builder.load(module.int_type(32), frame, 8);
        let invalid = module.append_block(dispatcher);
        let mut cases = Vec::new();
        for (k, &state) in declared[1..].iter().enumerate() {
            let block = module.append_block(dispatcher);
            builder.position(block);
            let returned = builder.call(state, &params);
            builder.ret(returned);
            cases.push((module.const_int(module.int_type(32), k as u128), block));
        }
        builder.position(entry);
        builder.switch(number, invalid, &cases);
        builder.position(invalid);
        let trap = module
            .intrinsic("llvm.trap", &[])
            .map_err(|what| CodegenError::Invalid {
                definition: header.definition.clone(),
                what,
            })?;
        builder.call(trap, &[]);
        builder.unreachable();
        for (k, &state) in declared[1..].iter().enumerate() {
            states.push((unit, plan, k as u32, state));
        }
    }
    Ok(states)
}
