//! Inactive physical planning for wrappers with entirely mechanical cleanup.
//! This is separate from ordinary plans: no caller or emitter selects it yet.
use super::*;
use crate::continuation::*;
use crate::stage::ValidatedUnit;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolatePlan {
    pub protocol: IsolateProtocol,
    pub cleanup: SynchronousCleanupPlan,
    pub frame: ContinuationPlan,
}

impl IsolateProtocol {
    /// Build a retained wrapper over already checked ordinary callee plans.
    /// Every incoming direct caller must prove the same callback and actual
    /// environment. Unknown cleanup still refuses the complete wrapper.
    pub fn frame(
        &self,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        ordinary: &ContinuationPlans,
    ) -> Result<IsolatePlan, String> {
        validate(units, inputs, ordinary).map_err(|e| e.to_string())?;
        let wrapper = self.checked_callers(units, inputs)?;
        let cleanup = self
            .synchronous_cleanup(wrapper, inputs.table)
            .ok_or("isolate cleanup requires a certified entered protocol")?;
        let index: BTreeMap<_, _> = units
            .iter()
            .map(|u| (u.unit().header.instance, u.unit()))
            .collect();
        let digests = index.iter().map(|(id, u)| (*id, u.digest())).collect();
        let suspends = ordinary.plans.keys().copied().collect();
        // The source-proved begin is entered, not an opaque retry. This
        // classification is local to the inactive proposal; ordinary callers
        // and wrapper plans keep their original classification.
        let classify = |definition: &hirsel_syntax::DefPath| {
            if definition.is_core("$intrinsic.IsolateBegin") {
                Some(CalleeClass::Boundary)
            } else if [
                "$intrinsic.IsolateEnd",
                "$intrinsic.IsolateCount",
                "$intrinsic.IsolateKind",
                "$intrinsic.IsolateRule",
                "$intrinsic.IsolateDefinition",
                "$intrinsic.IsolateLine",
                "$intrinsic.IsolateColumn",
                "$intrinsic.IsolateSpawn",
            ]
            .iter()
            .any(|path| definition.is_core(path))
            {
                // These reviewed task-local readers and End cannot suspend.
                Some(CalleeClass::Local)
            } else {
                (inputs.classify)(definition)
            }
        };
        let proposed = PlanInputs {
            classify: &classify,
            ..*inputs
        };
        let mut frame = plan_unit(
            wrapper.unit(),
            &proposed,
            &index,
            &suspends,
            &digests,
            &ordinary.plans,
        );
        if frame.representation != Representation::Resumable || !frame.cleanups.is_empty() {
            return Err(format!(
                "unsupported isolate frame: {:?}",
                frame.representation
            ));
        }
        let begin = frame
            .states
            .iter_mut()
            .find(|s| s.boundary.call == self.begin.call)
            .ok_or("missing isolate begin state")?;
        let runtime = self
            .begin_operation(wrapper, inputs.table, begin.bytes)
            .ok_or("unretained isolate begin mark")?;
        begin.bytes = runtime.results[0].offset + runtime.results[0].bytes;
        begin.action = ResumeAction::ResumeRuntime(self.begin.call);
        begin.runtime = Some(runtime);
        frame.cost.frame_bytes = frame.states.iter().map(|s| s.bytes).max().unwrap_or(0);
        if frame.cost.frame_bytes > MAX_FRAME_BYTES {
            return Err("oversize isolate frame".into());
        }
        let effects: Vec<_> = wrapper
            .unit()
            .tables
            .calls
            .iter()
            .map(|c| call_effect(wrapper.unit(), c, &index, &suspends, proposed.classify))
            .collect();
        frame.certificate = certify(
            wrapper.unit(),
            inputs,
            &effects,
            &digests,
            &frame.representation,
            (&frame.parameters, &frame.records),
            &frame.storage,
            &frame.states,
            &frame.cleanups,
            frame.captures,
        );
        Ok(IsolatePlan {
            protocol: self.clone(),
            cleanup,
            frame,
        })
    }

    pub(super) fn checked_callers<'a>(
        &self,
        units: &[&'a ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
    ) -> Result<&'a ValidatedUnit<Ready>, String> {
        let wrapper = units
            .iter()
            .copied()
            .find(|u| u.unit().header.instance == self.identity.wrapper)
            .ok_or("missing isolate wrapper")?;
        let callback_id = self
            .identity
            .target
            .instance
            .ok_or("unknown isolate callback")?;
        let callback = units
            .iter()
            .copied()
            .find(|u| u.unit().header.instance == callback_id)
            .ok_or("missing isolate callback")?;
        if inputs.environments.get(&callback_id).copied().flatten() != self.identity.environment {
            return Err("isolate callback environment mismatch".into());
        }
        let mut found = false;
        for caller in units {
            // A function-valued use could hide an incoming caller from this
            // direct-edge census. The specialized library instance may not
            // escape as code or a newly constructed callable.
            for inst in &caller.unit().instructions {
                let symbol = match *inst {
                    Inst::FunctionRef { symbol, .. } => Some(symbol),
                    Inst::Closure { closure, .. } => caller
                        .unit()
                        .tables
                        .closures
                        .get(closure.index())
                        .and_then(|c| c.code)
                        .map(|c| c.symbol),
                    _ => None,
                };
                if symbol
                    .and_then(|s| caller.unit().tables.symbols.get(s.index()))
                    .is_some_and(|s| s.instance == Some(self.identity.wrapper))
                {
                    return Err("isolate wrapper escapes its proved callers".into());
                }
            }
            for (id, call) in caller.unit().tables.calls.iter().enumerate() {
                let Some(symbol) = callee_symbol(caller.unit(), call) else {
                    continue;
                };
                if symbol.instance != Some(self.identity.wrapper) {
                    continue;
                }
                let at = CallId(id as u32);
                let identity = IsolateCallback::prove(
                    caller.unit(),
                    at,
                    wrapper.unit(),
                    callback.unit(),
                    self.identity.environment,
                    inputs.table,
                )
                .ok_or("unproved incoming isolate caller")?;
                let mut expected = self.identity.clone();
                expected.invocation = at;
                if identity != expected {
                    return Err("different incoming isolate callback".into());
                }
                if at == self.identity.invocation {
                    self.validate(caller, wrapper, callback, inputs.table)
                        .map_err(str::to_owned)?;
                    found = true;
                }
            }
        }
        if !found {
            return Err("missing original isolate caller".into());
        }
        Ok(wrapper)
    }
}

impl IsolatePlan {
    /// Reconstruct physical liveness, storage, loans and offsets independently
    /// of the producer. A valid digest never grants wrapper admission.
    pub fn check(
        &self,
        units: &[&ValidatedUnit<Ready>],
        inputs: &PlanInputs<'_>,
        ordinary: &ContinuationPlans,
    ) -> Result<(), String> {
        validate(units, inputs, ordinary).map_err(|e| e.to_string())?;
        let wrapper = self.protocol.checked_callers(units, inputs)?;
        let callback = units
            .iter()
            .copied()
            .find(|u| Some(u.unit().header.instance) == self.protocol.identity.target.instance)
            .ok_or("missing isolate callback")?;
        let mut checked = false;
        for caller in units {
            if self
                .protocol
                .validate(caller, wrapper, callback, inputs.table)
                .is_ok()
            {
                if !self.protocol.check_synchronous_cleanup(
                    caller,
                    wrapper,
                    callback,
                    inputs.table,
                    &self.cleanup,
                ) {
                    return Err("isolate mechanical cleanup coverage".into());
                }
                checked = true;
                break;
            }
        }
        if !checked {
            return Err("isolate source protocol".into());
        }
        super::super::validate::isolate_frame(units, inputs, ordinary, &self.protocol, &self.frame)
            .map_err(|e| e.to_string())
    }
}
