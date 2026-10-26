//! Resumable continuation alternates (r6 S7) of validated continuation plans.
//!
//! A resumable body gets one alternate per resume state and a dispatcher,
//! each with the body's own signature plus a trailing frame pointer. Declared
//! parameters, region records and storage live at any boundary live in the
//! frame; state 0 stores the parameters once. At each boundary the alternate
//! materializes the state's live values and writes the state into the frame
//! header before anything else: an operation boundary then asks
//! `hirsel_rt_v2_resume_point` whether to suspend (telling it whether a
//! non-head can issue the operation), and a resumable callee runs
//! through its own dispatcher in its nested frame. State k reloads its values
//! and reissues its boundary call, so no executed work is repeated.
use crate::function::{FnEmitter, Place, flags};
use crate::module::{Emitted, Emitter, Inputs, PTR};
use cranelift_codegen::Context;
use cranelift_codegen::ir::{self, InstBuilder, Value, condcodes::IntCC, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_lir::abi_types::ParameterPassing;
use hirsel_lir::continuation::{
    ContinuationPlan, ContinuationPlans, ExclusiveSlot, NestedFrame, Representation, ResumeAction,
    ResumeState, LiveOwnedSlot, resume_name, resume_site_hint,
};
use hirsel_lir::emission::{EmitInputs, artifact};
use hirsel_lir::ir::*;
use hirsel_lir::runtime_abi::{RuntimeEntry, STATUS_SUSPENDED};
use hirsel_lir::stage::{ObjectArtifact, ObjectSymbol, ObjectSymbolKind, Ready, ValidatedUnit};

pub(crate) struct Resume<'a> {
    pub(crate) plan: &'a ContinuationPlan,
    pub(crate) state: u32,
    pub(crate) frame: Option<Value>,
    /// The boundary call this alternate starts at: its protocol already ran.
    pub(crate) resuming: Option<CallId>,
}

impl FnEmitter<'_, '_, '_> {
    fn frame(&self) -> Value {
        self.resume
            .as_ref()
            .and_then(|r| r.frame)
            .expect("resume frame")
    }

    fn frame_place(
        &mut self,
        offset: u64,
        layout: hirsel_lir::layout_types::LayoutId,
    ) -> Emitted<Place> {
        let frame = self.frame();
        let align = self.layout(layout)?.align;
        Ok(Place {
            columns: None,
            ptr: self.offset(frame, offset),
            layout,
            align: align.min(crate::function::low_bit(offset).max(1)).min(16),
            block: None,
        })
    }

    /// Storage live at a boundary and every region record are frame-resident
    /// in every state.
    pub(crate) fn resume_storage(&mut self) -> Emitted {
        let Some(plan) = self.resume.as_ref().map(|r| r.plan) else {
            return Ok(());
        };
        for storage in &plan.storage {
            let place = self.frame_place(storage.offset, storage.layout)?;
            if !matches!(
                self.unit.slots[storage.slot.index()].kind,
                SlotKind::Capture(_)
            ) {
                self.slots[storage.slot.index()] = Some(place);
            }
        }
        if plan.records.len() != self.regions.len() {
            return self.invalid("a resumable plan without every region record");
        }
        for record in &plan.records {
            let frame = self.frame();
            self.regions[record.region.index()] = self.offset(frame, record.offset);
        }
        Ok(())
    }

    /// Declared parameters, materialized once: state 0 stores each incoming
    /// value (an address parameter's bytes) in its frame record, and every
    /// state binds the parameter from that record, never from a re-entry's
    /// arguments, which the caller passes again unchanged. An exclusive
    /// parameter's record holds the caller slot's address, and its slot is that
    /// place: every access reaches the caller's canonical bytes.
    pub(crate) fn resume_parameters(&mut self) -> Emitted {
        let (plan, state) = {
            let resume = self.resume.as_ref().unwrap();
            (resume.plan, resume.state)
        };
        let abi = self.sig.abi;
        if plan.parameters.len() != abi.parameters.len() {
            return self.invalid("a resumable plan off its body's parameters");
        }
        for (i, (parameter, record)) in abi.parameters.iter().zip(&plan.parameters).enumerate() {
            if parameter.passing == ParameterPassing::ExclusiveAddress {
                let (Some(slot), ParameterPassing::ExclusiveAddress) =
                    (record.slot, record.passing)
                else {
                    return self
                        .invalid("an exclusive parameter without its slot's address record");
                };
                let frame = self.frame();
                let at = self.offset(frame, record.offset);
                if state == 0 {
                    let incoming = self.entry_params[self.sig.first[i]];
                    self.b.ins().store(flags(), incoming, at, 0);
                }
                let ptr = self.load_ptr(at);
                let align = self.layout(record.layout)?.align;
                self.slots[slot.index()] = Some(Place {
                    columns: None,
                    ptr,
                    layout: record.layout,
                    align,
                    block: None,
                });
                continue;
            }
            let shape = self.layout_shape(record.layout)?;
            let place = self.frame_place(record.offset, record.layout)?;
            if state == 0 {
                let first = self.sig.first[i];
                let value = match parameter.passing {
                    ParameterPassing::ByValue => {
                        let args = self.entry_params[first..first + parameter.slots.len()].to_vec();
                        self.assemble(record.layout, &args)?
                    }
                    ParameterPassing::ReadOnlyAddress | ParameterPassing::TransferAddress => {
                        let incoming = Place {
                            columns: None,
                            ptr: self.entry_params[first],
                            layout: record.layout,
                            align: self.layout(record.layout)?.align,
                            block: None,
                        };
                        self.load(shape, incoming)
                    }
                    _ => return self.invalid("a resumable parameter the frame cannot hold"),
                };
                self.store(shape, value, place)?;
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

    /// A resumable task body's ordinary entry: the runtime runs its dispatcher
    /// over an owned frame, saving that frame at each suspension.
    pub(crate) fn emit_resume_entry(mut self, plan: &ContinuationPlan) -> Emitted {
        let [context, environment, caller, site] = self.entry_params[..] else {
            return self.invalid("a resumable task body off the task entry signature");
        };
        let header = &self.unit.header;
        let name = resume_name(&header.definition, header.instance, None);
        let mut signature = self.sig.ir.clone();
        signature.params.push(ir::AbiParam::new(PTR));
        self.em
            .manifest_import(&name, ObjectSymbolKind::Function, None, None);
        let id = self.em.import_function(&name, &signature)?;
        let dispatcher = self.func_addr(id);
        let Ok(bytes) = i64::try_from(plan.cost.frame_bytes) else {
            return self.invalid("a resumable frame beyond its bound");
        };
        let bytes = self.b.ins().iconst(types::I32, bytes);
        let status = self.rt1(
            RuntimeEntry::ResumeRun,
            &[context, environment, caller, site, dispatcher, bytes],
        )?;
        self.b.ins().return_(&[status]);
        self.b.seal_all_blocks();
        let config = self.em.module.target_config();
        self.b.finalize(config);
        Ok(())
    }

    /// Store a state's live values and number before its boundary call.
    fn materialize(&mut self, state: &ResumeState) -> Emitted {
        self.materialize_live(&state.live,state.target.state)
    }
    fn materialize_live(&mut self,live:&[LiveOwnedSlot],number:u32)->Emitted {
        for live in live {
            if self.versions_contains(live.value) {
                continue;
            }
            let shape = self.value_shape(live.value)?;
            let value = self.get(live.value)?;
            let place = self.frame_place(live.offset, live.layout)?;
            self.store(shape, value, place)?;
        }
        let frame = self.frame();
        let number = self
            .b
            .ins()
            .iconst(types::I32, i64::from(number));
        self.b.ins().store(flags(), number, frame, 0);
        Ok(())
    }

    /// Define a state's live values from the frame; bytes get fresh copies.
    pub(crate) fn reload(&mut self, state: &ResumeState) -> Emitted {
        self.reload_live(&state.live)
    }
    pub(crate) fn reload_live(&mut self,live:&[LiveOwnedSlot])->Emitted {
        for live in live {
            if self.versions_contains(live.value) {
                continue;
            }
            let shape = self.value_shape(live.value)?;
            let place = self.frame_place(live.offset, live.layout)?;
            let value = self.load(shape, place);
            self.define(live.value, value)?;
        }
        Ok(())
    }

    fn suspend(&mut self) -> Emitted {
        self.return_suspended()
    }

    /// Whether `call` is a boundary of this body's resumable plan.
    /// The proof that nested boundary `call` passes callee parameter `source`
    /// as a slot of this frame.
    pub(crate) fn resume_exclusive(&self, call: CallId, source: u32) -> Option<ExclusiveSlot> {
        let plan = self.resume.as_ref()?.plan;
        plan.states
            .iter()
            .find(|s| s.boundary.call == call)?
            .exclusive
            .clone()
            .filter(|e| e.call == call && e.parameter == source)
    }

    /// The stable address of a proved exclusive slot: its offset in this
    /// frame's allocation, never a temporary.
    pub(crate) fn exclusive_slot_address(&mut self, proof: &ExclusiveSlot) -> Emitted<Value> {
        let bound = self.slots.get(proof.slot.index()).copied().flatten();
        let stored = self.resume.as_ref().is_some_and(|r| {
            r.plan
                .storage
                .iter()
                .any(|s| s.slot == proof.slot && s.offset == proof.offset)
        });
        if !stored || !bound.is_some_and(|p| p.columns.is_none() && p.layout == proof.layout) {
            return self.invalid("an exclusive slot outside this frame's storage");
        }
        let frame = self.frame();
        Ok(self.offset(frame, proof.offset))
    }

    pub(crate) fn resume_boundary_call(&self, call: CallId) -> bool {
        self.resume
            .as_ref()
            .is_some_and(|r| r.plan.states.iter().any(|s| s.boundary.call == call))
    }

    /// The boundary protocol before `call`; for a resumable callee, its frame.
    pub(crate) fn resume_boundary(
        &mut self,
        call: CallId,
    ) -> Emitted<Option<(Value, NestedFrame)>> {
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
        let frame = self.frame();
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
            let nested = self.offset(frame, callee.offset);
            if !resuming {
                self.materialize(state)?;
                let zero = self.b.ins().iconst(types::I32, 0);
                self.b.ins().store(flags(), zero, nested, 0);
            }
            return Ok(Some((nested, callee)));
        }
        if !resuming {
            self.materialize(state)?;
            let hint = resume_site_hint(self.unit, state);
            let number = self
                .b
                .ins()
                .iconst(types::I32, i64::from(state.target.state | hint));
            let decided = self.rt1(RuntimeEntry::ResumePoint, &[self.context, frame, number])?;
            let (suspended, proceed) = (self.b.create_block(), self.b.create_block());
            self.b.set_cold_block(suspended);
            self.b.ins().brif(decided, suspended, &[], proceed, &[]);
            self.b.switch_to_block(suspended);
            self.suspend()?;
            self.b.switch_to_block(proceed);
        }
        Ok(None)
    }

    pub(crate) fn runtime_result(
        &mut self,
        call: CallId,
        layout: hirsel_lir::LayoutId,
    ) -> Emitted<Option<Place>> {
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

    pub(crate) fn bind_runtime(
        &mut self,
        call: CallId,
        name: Option<&str>,
        codes: (Value, Value),
    ) -> Emitted {
        let Some((plan, state)) = self.resume.as_ref().and_then(|r| {
            r.plan
                .states
                .iter()
                .find(|s| s.boundary.call == call)
                .map(|s| (r.plan, s))
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
        let frame = self.frame();
        let number = self
            .b
            .ins()
            .iconst(types::I32, i64::from(state.target.state));
        let kind = self
            .b
            .ins()
            .iconst(types::I32, i64::from(runtime.kind.tag()));
        let bytes = self
            .b
            .ins()
            .iconst(types::I32, plan.cost.frame_bytes as i64);
        self.rt(
            RuntimeEntry::ResumeTail,
            &[self.context, frame, number, kind, bytes, codes.0, codes.1],
        )
        .map(|_| ())
    }

    pub(crate) fn resume_tail_bind(&mut self, call: CallId) -> Emitted {
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
        let number = self
            .b
            .ins()
            .iconst(types::I32, i64::from(state.target.state));
        let kind = self
            .b
            .ins()
            .iconst(types::I32, state.tail.as_ref().unwrap().kind as i64);
        let bytes = self
            .b
            .ins()
            .iconst(types::I32, resume.plan.cost.frame_bytes as i64);
        let site = &self.unit.tables.calls[call.index()];
        let code = self.anchor_code(site.anchor)?;
        let codes = self.caller_sites(code, site.forward_caller);
        let frame = self.frame();
        self.rt(
            RuntimeEntry::ResumeTail,
            &[self.context, frame, number, kind, bytes, codes.0, codes.1],
        )?;
        Ok(())
    }

    pub(crate) fn resume_cleanup_bind(&mut self,block:BlockId,instruction:usize)->Emitted {
        let Some(resume)=self.resume.as_ref() else {return Ok(())};
        let Some(state)=resume.plan.cleanups.iter().find(|s|s.block==block&&s.instruction==instruction as u32) else {return Ok(())};
        let (state,bytes)=(state.clone(),resume.plan.cost.frame_bytes);
        self.materialize_live(&state.live,state.state)?;
        let number=self.b.ins().iconst(types::I32,i64::from(state.state));
        let tag = if matches!(self.unit.types[self.unit.values[state.value.index()].ty.index()].repr, ValueRepr::Block) {
            if matches!(self.layout(state.layout)?.kind, hirsel_lir::layout_types::LayoutKind::Shared { .. }) { 48 } else { 49 }
        } else { 50 };
        let kind=self.b.ins().iconst(types::I32,tag);
        let bytes=self.b.ins().iconst(types::I32,bytes as i64);
        let code=self.cleanup_code(state.anchor)?;
        let (anchor,site)=self.failure_sites(code);
        let frame=self.frame();
        self.rt(RuntimeEntry::ResumeTail,&[self.context,frame,number,kind,bytes,anchor,site])?;
        Ok(())
    }

    /// After a nested resumable call: a suspended callee suspends this frame,
    /// whose state the boundary already recorded.
    pub(crate) fn resume_propagate(&mut self, status: Value) -> Emitted {
        let suspended = self
            .b
            .ins()
            .icmp_imm(IntCC::Equal, status, i64::from(STATUS_SUSPENDED));
        let (cold, next) = (self.b.create_block(), self.b.create_block());
        self.b.set_cold_block(cold);
        self.b.ins().brif(suspended, cold, &[], next, &[]);
        self.b.switch_to_block(cold);
        self.suspend()?;
        self.b.switch_to_block(next);
        Ok(())
    }
}

/// Every resumable plan's alternates and dispatcher, in one object.
pub(crate) fn emit(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &ContinuationPlans,
) -> Emitted<ObjectArtifact> {
    hirsel_lir::continuation::validate_runtime_operations(
        units,
        inputs.layouts,
        inputs.target,
        plans,
    )
    .map_err(|error| crate::CodegenError::Invalid {
        definition: units
            .first()
            .expect("entered runtime units")
            .unit()
            .header
            .definition
            .clone(),
        what: error.to_string(),
    })?;
    let mut emitter = Emitter::new(Inputs::units(inputs), false)?;
    let mut bodies = Vec::new();
    for unit in units {
        let unit = unit.unit();
        let header = &unit.header;
        let Some(plan) = plans
            .get(header.instance)
            .filter(|p| p.representation == Representation::Resumable)
        else {
            continue;
        };
        let abi = emitter.fn_abi(header.abi, &header.definition)?;
        let mut sig = emitter.entry_signature(abi, &header.definition)?;
        sig.ir.params.push(ir::AbiParam::new(PTR));
        let mut ids = Vec::new();
        for state in std::iter::once(None).chain((0..=(plan.states.len()+plan.cleanups.len()) as u32).map(Some)) {
            let name = resume_name(&header.definition, header.instance, state);
            let id = emitter.declare_function(&name, Linkage::Export, &sig.ir)?;
            emitter.manifested.exports.insert(
                name.clone(),
                ObjectSymbol {
                    name: name.clone(),
                    kind: ObjectSymbolKind::Function,
                    definition: Some(header.definition.clone()),
                    abi: None,
                },
            );
            ids.push((name, id));
        }
        bodies.push((unit, plan, sig, ids));
    }
    let mut context = Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    for (unit, plan, sig, ids) in bodies {
        for (state, (name, id)) in ids.iter().enumerate().skip(1) {
            context.func =
                ir::Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
            {
                let builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
                let mut function = FnEmitter::new(&mut emitter, unit, builder, sig.clone());
                function.resume = Some(Resume {
                    plan,
                    state: state as u32 - 1,
                    frame: None,
                    resuming: None,
                });
                function.emit()?;
            }
            emitter.compile(name, *id, &mut context)?;
            emitter.module.clear_context(&mut context);
        }
        // The dispatcher enters the state its frame header names.
        context.func =
            ir::Function::with_name_signature(ir::UserFuncName::default(), sig.ir.clone());
        {
            let mut b = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let params = b.block_params(entry).to_vec();
            let frame = *params.last().unwrap();
            let state = b.ins().load(types::I32, flags(), frame, 0);
            let invalid = b.create_block();
            let mut table = Vec::new();
            for (_, id) in ids.iter().skip(1) {
                let block = b.create_block();
                table.push((block, *id));
            }
            let data = ir::JumpTableData::new(
                b.func.dfg.block_call(invalid, &[]),
                &table
                    .iter()
                    .map(|(block, _)| b.func.dfg.block_call(*block, &[]))
                    .collect::<Vec<_>>(),
            );
            let jt = b.create_jump_table(data);
            b.ins().br_table(state, jt);
            for (block, id) in table {
                b.switch_to_block(block);
                let callee = emitter.module.declare_func_in_func(id, b.func);
                let call = b.ins().call(callee, &params);
                let results = b.inst_results(call).to_vec();
                b.ins().return_(&results);
            }
            b.switch_to_block(invalid);
            b.ins().trap(crate::function::TRAP);
            b.seal_all_blocks();
            b.finalize(emitter.module.target_config());
        }
        let (name, id) = &ids[0];
        emitter.compile(name, *id, &mut context)?;
        emitter.module.clear_context(&mut context);
    }
    if let Some(first) = units.first() {
        emitter.drain_helpers(&first.unit().header.definition)?;
    }
    emitter.manifested.settle_imports();
    let Emitter {
        module, manifested, ..
    } = emitter;
    let bytes = crate::object::finish(module.finish())?;
    let file = hirsel_lir::object::ObjectFile::read(inputs.target, bytes)
        .map_err(|message| crate::CodegenError::Object { message })?;
    artifact(file, manifested, inputs, crate::TOOLCHAIN)
        .map_err(|problem| crate::CodegenError::Manifest { problem })
}
