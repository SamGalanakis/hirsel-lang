//! Resumable continuation alternates (r6 S7) of validated continuation plans.
//!
//! A resumable body gets one alternate per resume state and a dispatcher,
//! each with the body's own signature plus a trailing frame pointer. Storage
//! live at any boundary lives in the frame. At each boundary the alternate
//! materializes the state's live values and writes the state into the frame
//! header before anything else: an operation boundary then asks
//! `hirsel_rt_v2_resume_point` whether to suspend, and a resumable callee runs
//! through its own dispatcher in its nested frame. State k reloads its values
//! and reissues its boundary call, so no executed work is repeated.
use crate::function::{FnEmitter, Place, flags};
use crate::module::{Emitted, Emitter, Inputs, PTR};
use cranelift_codegen::ir::{self, InstBuilder, Value, condcodes::IntCC, types};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use hirsel_lir::continuation::{
    ContinuationPlan, ContinuationPlans, NestedFrame, Representation, ResumeState, resume_name,
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
        self.resume.as_ref().and_then(|r| r.frame).expect("resume frame")
    }

    fn frame_place(&mut self, offset: u64, layout: hirsel_lir::layout_types::LayoutId) -> Emitted<Place> {
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

    /// Storage live at a boundary is frame-resident in every state.
    pub(crate) fn resume_storage(&mut self) -> Emitted {
        let Some(plan) = self.resume.as_ref().map(|r| r.plan) else {
            return Ok(());
        };
        for storage in &plan.storage {
            let place = self.frame_place(storage.offset, storage.layout)?;
            match self.unit.slots[storage.slot.index()].kind {
                SlotKind::Parameter(_) | SlotKind::Capture(_) => {}
                _ => self.slots[storage.slot.index()] = Some(place),
            }
        }
        Ok(())
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
        for live in &state.live {
            if self.versions_contains(live.value) {
                continue;
            }
            let shape = self.value_shape(live.value)?;
            let value = self.get(live.value)?;
            let place = self.frame_place(live.offset, live.layout)?;
            self.store(shape, value, place)?;
        }
        let frame = self.frame();
        let number = self.b.ins().iconst(types::I32, i64::from(state.target.state));
        self.b.ins().store(flags(), number, frame, 0);
        Ok(())
    }

    /// Define a state's live values from the frame; bytes get fresh copies.
    pub(crate) fn reload(&mut self, state: &ResumeState) -> Emitted {
        for live in &state.live {
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

    /// The boundary protocol before `call`; for a resumable callee, its frame.
    pub(crate) fn resume_boundary(&mut self, call: CallId) -> Emitted<Option<(Value, NestedFrame)>> {
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
        if let Some(callee) = state.callee {
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
            let number = self.b.ins().iconst(types::I32, i64::from(state.target.state));
            let decided = self
                .rt1(RuntimeEntry::ResumePoint, &[self.context, frame, number])?;
            let (suspended, proceed) = (self.b.create_block(), self.b.create_block());
            self.b.set_cold_block(suspended);
            self.b.ins().brif(decided, suspended, &[], proceed, &[]);
            self.b.switch_to_block(suspended);
            self.suspend()?;
            self.b.switch_to_block(proceed);
        }
        Ok(None)
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
        for state in std::iter::once(None).chain((0..=plan.states.len() as u32).map(Some)) {
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
