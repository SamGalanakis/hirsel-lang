//! Continuation plans (r6 S7) for a Ready program, classified by the reviewed
//! runtime operation descriptors.
use super::*;
use hirsel_lir::continuation::{CalleeClass, ContinuationPlans, PlanInputs};
use hirsel_rt_operations::descriptors::{self as d, Locality, Transition};
use hirsel_syntax::{ContentHash, DefPath};

/// Startup, state getter, handoff and depth-leaf call extents composed into
/// every ahead root's final frame bound.
pub const AHEAD_HELPER_BYTES: [u64; 4] = [16, 8, 64, hirsel_rt::DEPTH_LEAF_STACK_BYTES];

/// The final-frame certificate of every ahead root the objects prove.
pub fn ahead_certificates(
    program: &ReadyProgram,
    objects: &[hirsel_lir::stage::ObjectArtifact],
) -> Vec<hirsel_lir::continuation::AheadCertificate> {
    let plan = super::ahead::plan(program);
    let plans = plans(program);
    let Some(evidence) = objects.iter().find_map(|o| o.ahead.as_ref()) else {
        return Vec::new();
    };
    plan.roots
        .keys()
        .filter_map(|root| {
            hirsel_lir::continuation::AheadCertificate::derive(
                &plan,
                &plans,
                *root,
                evidence,
                AHEAD_HELPER_BYTES,
            )
        })
        .collect()
}

/// The resume protocol and switch the plans are bound to.
pub fn runtime_identity() -> ContentHash {
    ContentHash::of_parts(
        "hirsel.continuation.runtime.v1",
        [
            hirsel_rt::CONTINUATION_PROTOCOL,
            hirsel_rt::AHEAD_SWITCH,
            crate::COMPILER_IDENTITY.as_slice(),
        ],
    )
}

/// A core operation with a reviewed descriptor is classified by its ops'
/// transitions; any other core runtime entry by its locality. Everything else
/// defers to its Ready body.
pub fn classify(definition: &DefPath) -> Option<CalleeClass> {
    let name = hirsel_lir::continuation::core_name(definition)?;
    let point = |ops: &[&str]| {
        ops.iter().any(|op| {
            d::op_descriptor(op)
                .is_none_or(|o| o.transitions.iter().any(|t| *t != Transition::Keep))
        })
    };
    if let Some((_, ops)) = d::REGISTRY.iter().find(|(n, _)| *n == name) {
        return Some(if point(ops) {
            CalleeClass::Boundary
        } else {
            CalleeClass::Local
        });
    }
    let symbol = format!("hirsel_rt_v2_{}", name.replace('.', "_"));
    let entry = d::entry_descriptor(&symbol)?;
    Some(match entry.locality {
        Locality::Local | Locality::Immutable | Locality::Concurrent => CalleeClass::Local,
        Locality::Op if point(entry.ops) => CalleeClass::Boundary,
        Locality::Op => CalleeClass::Local,
        Locality::Admission | Locality::Physical => CalleeClass::Unknown,
    })
}

fn units(
    program: &ReadyProgram,
) -> Vec<&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>> {
    program.units.iter().collect()
}

/// Every resumable plan's state alternates and dispatcher, in one object.
pub(super) fn objects(
    program: &ReadyProgram,
    backend: super::emit::Backend,
    opt: hirsel_lir::emission::OptLevel,
    measurement: bool,
) -> Result<Vec<hirsel_lir::stage::ObjectArtifact>, NativeError> {
    let plans = plans(program);
    if !plans.resumable() {
        return Ok(Vec::new());
    }
    // Entered-runtime certificates include incoming callable provenance from
    // callers that may themselves remain stackful. Emitters check this full
    // set, then emit alternates only for Resumable bodies.
    let units = units(program);
    let mut ahead = super::ahead::plan(program);
    if !hirsel_lir::ahead::validate_cpu_workers(
        &units,
        &program.table,
        &program.target,
        &ahead,
        program.fuel,
    ) {
        return Err(NativeError::new(
            StageName::Emission,
            "invalid CPU ahead certificate",
        ));
    }
    // Compact map callers need the same certified start and worker spawn
    // selection as their ordinary entries. Keep other spawn policies intact.
    ahead
        .spawns
        .retain(|_, root| ahead.roots.get(root).is_some_and(|root| root.cpu.is_some()));
    static EMPTY: std::collections::BTreeSet<ContentHash> = std::collections::BTreeSet::new();
    let inputs = hirsel_lir::emission::EmitInputs {
        ahead: Some(&ahead),
        continuation: Some(&plans),
        alternate: false,
        index: None,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &program.inline_instances,
        available_externally: &EMPTY,
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt,
        measurement,
        fuel: program.fuel,
        verify_backend: program.validation == super::stages::Validation::EveryStage,
    };
    let object = match backend {
        super::emit::Backend::Cranelift => {
            hirsel_cranelift::emit_resume_object(&units, &inputs, &plans)
                .map_err(|e| NativeError::new(StageName::Emission, e.to_string()))?
        }
        super::emit::Backend::Llvm => hirsel_llvm::emit_resume_object(&units, &inputs, &plans)
            .map_err(|e| NativeError::new(StageName::Emission, e.to_string()))?,
    };
    Ok(vec![object])
}

/// Derive the program's plans from its complete Ready bodies.
pub fn plans(program: &ReadyProgram) -> ContinuationPlans {
    let runtime = runtime_identity();
    ContinuationPlans::build(&units(program), &inputs(program, &runtime))
}

/// Independently validate imported plans against the same program.
pub fn validate(
    program: &ReadyProgram,
    plans: &ContinuationPlans,
) -> Result<(), hirsel_lir::continuation::ContinuationError> {
    let runtime = runtime_identity();
    hirsel_lir::continuation::validate(&units(program), &inputs(program, &runtime), plans)
}

fn inputs<'a>(program: &'a ReadyProgram, runtime: &ContentHash) -> PlanInputs<'a> {
    PlanInputs {
        table: &program.table,
        target: &program.target,
        environments: &program.ahead_environments,
        runtime: *runtime,
        classify: &classify,
    }
}
