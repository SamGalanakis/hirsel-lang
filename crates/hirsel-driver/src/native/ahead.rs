use super::*;
use hirsel_lir::ahead::AheadPlan;
use hirsel_lir::continuation::ContinuationPlans;
use hirsel_syntax::ContentHash;
use std::collections::BTreeMap;

pub(super) fn helper_identity() -> ContentHash {
    // This names the actual helper contract embedded in ahead metadata. The
    // complete compiler identity belongs to object/cache producer binding, not
    // to a link symbol: an unrelated, disabled diagnostic module must not alter
    // otherwise identical native code. AheadPlan additionally binds every body,
    // layout, target, fuel choice and capture recipe used by these helpers.
    ContentHash::of_parts(
        "hirsel.ahead.helpers.v6",
        [
            hirsel_rt::AHEAD_HELPERS,
            hirsel_rt::AHEAD_HOST_HELPERS,
            hirsel_rt::AHEAD_CPU_HELPERS,
            hirsel_rt::AHEAD_CPU_ALLOCATION_HELPERS,
            hirsel_rt::AHEAD_CPU_HANDOFF_HELPERS,
            hirsel_rt::AHEAD_SWITCH,
            if hirsel_llvm::NATIVE_TSAN || hirsel_cranelift::NATIVE_TSAN {
                hirsel_rt::TSAN_CALLBACK_BRIDGES
            } else {
                &[]
            },
            hirsel_rt::DEPTH_HELPERS,
            hirsel_rt::DEPTH_LEAF_STACK_BYTES.to_le_bytes().as_slice(),
        ],
    )
}
pub fn plan(program: &ReadyProgram) -> AheadPlan {
    let units: Vec<_> = program.units.iter().collect();
    AheadPlan::build(
        &units,
        &program.table,
        &program.target,
        &program.ahead_environments,
        program.fuel,
        helper_identity(),
    )
}

/// Whether a body's ahead code also has its resumable state-0 alternate.
fn resumable(plans: &ContinuationPlans, id: ContentHash) -> bool {
    plans
        .get(id)
        .is_some_and(|p| p.representation == hirsel_lir::continuation::Representation::Resumable)
}

/// A body's final frame: the larger of its alternates', since a root enters
/// either. A resumable body without both final frames has none.
pub(in crate::native) fn body_frame(
    plan: &AheadPlan,
    plans: &ContinuationPlans,
    id: ContentHash,
    frames: &std::collections::BTreeMap<String, u64>,
) -> Option<u64> {
    let symbol = plan.symbols.get(&id)?;
    let ordinary = *frames.get(&hirsel_lir::ahead::alternate_name(&symbol.definition, id))?;
    if !resumable(plans, id) {
        return Some(ordinary);
    }
    let resumed = *frames.get(&hirsel_lir::ahead::alternate_resume_name(
        &symbol.definition,
        id,
    ))?;
    Some(ordinary.max(resumed))
}

pub(in crate::native) fn inputs<'a>(
    program: &'a ReadyProgram,
    plan: &'a AheadPlan,
    plans: &'a ContinuationPlans,
    opt: hirsel_lir::emission::OptLevel,
) -> hirsel_lir::emission::EmitInputs<'a> {
    static EMPTY: std::collections::BTreeSet<ContentHash> = std::collections::BTreeSet::new();
    hirsel_lir::emission::EmitInputs {
        ahead: Some(plan),
        continuation: Some(plans),
        alternate: true,
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
        measurement: false,
        fuel: program.fuel,
        verify_backend: program.validation == super::stages::Validation::EveryStage,
    }
}
pub(in crate::native) fn unknown_machine_calls(
    object: &hirsel_lir::stage::ObjectArtifact,
    plan: &AheadPlan,
    plans: &ContinuationPlans,
    program: &ReadyProgram,
) -> bool {
    use hirsel_lir::{ir::*, stage::ObjectSymbolKind};
    let mut prefix_runtime = std::collections::BTreeSet::new();
    let mut prefix_functions = std::collections::BTreeSet::new();
    if plan
        .roots
        .values()
        .any(|r| r.host.is_some() || r.cpu.is_some())
    {
        // Every unclassified instruction and every non-scalar call in these
        // exact bodies barriers before its lowering. Accept explicit, known
        // runtime declarations there; machine-created libc/outline helpers
        // have no such source site and remain refused.
        prefix_runtime.extend(
            hirsel_lir::runtime_abi::RuntimeEntry::ALL
                .iter()
                .map(|e| e.declaration().symbol.to_owned()),
        );
        let all: Vec<_> = program.units.iter().collect();
        for unit in all {
            let u = unit.unit();
            let Some(spine) = plan.host_spine(u.header.instance) else {
                continue;
            };
            for (i, inst) in u.instructions.iter().enumerate() {
                if let Inst::FunctionRef { symbol, .. } = inst
                    && !spine.private_instructions[i]
                {
                    // This exact callable construction is behind the host
                    // instruction barrier. Its ordinary thunk is neither a
                    // private callee nor permission for an unknown helper.
                    let symbol = &u.tables.symbols[symbol.index()];
                    prefix_functions.insert(hirsel_lir::emission::function_thunk_name(
                        &symbol.definition,
                        symbol.instance.as_ref(),
                    ));
                }
            }
            for (i, call) in u.tables.calls.iter().enumerate() {
                if spine.scalar_calls.contains(&CallId(i as u32)) {
                    continue;
                }
                match call.callee {
                    Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => {
                        let symbol = &u.tables.symbols[symbol.index()];
                        if symbol
                            .instance
                            .is_none_or(|id| !plan.has_private_edge(u.header.instance, id))
                        {
                            prefix_functions.insert(hirsel_lir::emission::symbol_name(
                                &symbol.definition,
                                symbol.instance.as_ref(),
                            ));
                        }
                    }
                    Callee::Operation(op) => {
                        let operation = &u.tables.operations[op.index()];
                        if let Some(row) = program.manifest.row(operation)
                            && let hirsel_core_contract::Disposition::RuntimePrimitive(template) =
                                row.dispositions.get(hirsel_core_contract::Engine::Native)
                            && let Some(name) =
                                hirsel_lir::abi::resolve_call_symbol(template, u, call)
                        {
                            prefix_runtime.insert(name);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    object.imports.iter().any(|s| match s.kind {
        ObjectSymbolKind::Data => false,
        ObjectSymbolKind::Function => {
            !prefix_functions.contains(&s.name)
                && !plan.bodies.keys().any(|id| {
                    let symbol = &plan.symbols[id];
                    s.name == hirsel_lir::ahead::alternate_name(&symbol.definition, *id)
                        || resumable(plans, *id)
                            && s.name
                                == hirsel_lir::ahead::alternate_resume_name(&symbol.definition, *id)
                })
        }
        ObjectSymbolKind::Runtime => {
            !prefix_runtime.contains(&s.name)
                && !((hirsel_llvm::NATIVE_TSAN || hirsel_cranelift::NATIVE_TSAN)
                    && hirsel_core_contract::tsan::private_stack_bytes(&s.name)
                        .is_some_and(|bytes| bytes <= 64))
                && !matches!(
                    s.name.as_str(),
                    "hirsel_rt_v2_ahead_state"
                        | "hirsel_rt_v2_ahead_handoff"
                        | "hirsel_rt_v2_resume_point"
                        | "hirsel_rt_v2_depth_enter_at_site"
                        | "hirsel_rt_v2_depth_leave"
                        | "hirsel_rt_v2_failure_begin"
                        | "hirsel_rt_v2_failure_reanchor"
                        | "hirsel_rt_v2_release"
                        | "hirsel_rt_v2_finalize"
                        | "hirsel_rt_v2_task_finish"
                )
        }
        _ => true,
    })
}

/// Validate sidecar/object/plan coherence before trusting a metadata object.
/// LLVM frame records are read again from the exact bytes. Cranelift's public
/// final frame data is retained in the same object sidecar, never borrowed from
/// a cached ordinary unit. Missing or corrupt sidecars cannot publish entries.
pub fn check_objects(
    program: &ReadyProgram,
    objects: &[hirsel_lir::stage::ObjectArtifact],
) -> Result<(), NativeError> {
    let plan = plan(program);
    check_objects_with_plan(program, objects, &plan)
}

pub(super) fn check_objects_with_plan(
    program: &ReadyProgram,
    objects: &[hirsel_lir::stage::ObjectArtifact],
    plan: &AheadPlan,
) -> Result<(), NativeError> {
    let plans = super::continuation::plans(program);
    let fail =
        |reason: &str| NativeError::new(StageName::Link, format!("ahead certificate: {reason}"));
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    for metadata in objects.iter().filter(|o| {
        o.exports
            .iter()
            .any(|s| s.name.starts_with("hirsel_ahead_"))
    }) {
        let bounds = if let Some(e) = &metadata.ahead {
            if e.plan != plan.identity {
                return Err(fail("stale plan/body/layout/fuel/helper identity"));
            }
            let code = objects
                .iter()
                .find(|o| ContentHash::of_bytes(&o.bytes) == e.object)
                .ok_or_else(|| fail("missing exact alternate object"))?;
            if code.ahead.as_ref() != Some(e)
                || code.reproducibility.options != e.options
                || metadata.reproducibility.options != e.options
            {
                return Err(fail("corrupt object/frame sidecar"));
            }
            let frames: BTreeMap<_, _> = e.frames.iter().copied().collect();
            if frames.len() != e.frames.len()
                || frames.keys().any(|id| !plan.bodies.contains_key(id))
            {
                return Err(fail("unknown or duplicate machine frame"));
            }
            if code.reproducibility.toolchain.starts_with("llvm ") {
                let actual = hirsel_lir::object::llvm_stack_sizes(&code.bytes)
                    .map_err(|_| fail("malformed final LLVM frames"))?;
                for (id, frame) in &frames {
                    if !plan.symbols.contains_key(id) {
                        return Err(fail("unknown frame body"));
                    }
                    if body_frame(&plan, &plans, *id, &actual) != Some(*frame) {
                        return Err(fail("stale final LLVM frame"));
                    }
                }
            }
            let bounds: BTreeMap<_, _> = plan
                .roots
                .iter()
                .filter_map(|(id, root)| {
                    if unknown_machine_calls(code, &plan, &plans, program) {
                        return None;
                    }
                    hirsel_lir::ahead::stack_bound_with_depth(
                        &root.graph,
                        &frames,
                        16,
                        8,
                        root.handoff_bytes(64),
                        hirsel_rt::DEPTH_LEAF_STACK_BYTES,
                    )
                    .map(|n| (*id, n))
                })
                .collect();
            if bounds.iter().map(|(id, n)| (*id, *n)).collect::<Vec<_>>() != e.bounds {
                return Err(fail("invalid frame DAG bound"));
            }
            for root in bounds.keys() {
                for id in plan.roots[root].graph.bodies.keys() {
                    let definition = &plan.symbols[id].definition;
                    let mut names = vec![hirsel_lir::ahead::alternate_name(definition, *id)];
                    if resumable(&plans, *id) {
                        names.push(hirsel_lir::ahead::alternate_resume_name(definition, *id));
                    }
                    for name in names {
                        if objects
                            .iter()
                            .flat_map(|o| &o.exports)
                            .filter(|s| s.name == name)
                            .count()
                            != 1
                        {
                            return Err(fail("missing or duplicate real alternate definition"));
                        }
                    }
                }
            }
            bounds
        } else {
            BTreeMap::new()
        };
        let expected = hirsel_cranelift::emit_ahead_metadata(
            &inputs(
                program,
                &plan,
                &plans,
                hirsel_lir::emission::OptLevel::Default,
            ),
            &plan,
            &bounds,
            &registry,
        )
        .map_err(|_| fail("could not validate immutable metadata"))?;
        if metadata.bytes != expected.bytes
            || metadata.exports != expected.exports
            || metadata.imports != expected.imports
        {
            return Err(fail(
                "metadata does not bind the exact recipe/frame sidecar",
            ));
        }
    }
    Ok(())
}
