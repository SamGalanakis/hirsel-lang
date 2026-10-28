//! The emission pipeline: validated Ready units to one owned object.
//!
//! `emit_object` checks every unit's receipt against the immutable inputs, cross-checks
//! LLVM's DataLayout against `TargetSpec`, builds one module, runs the verifier before and
//! after the optimization pipeline, generates the object, reads its symbol interface back
//! from the ELF bytes and compares it with the manifest the emitter declared. Every LLVM
//! handle and every borrowed LIR table is dropped before the `ObjectArtifact` is returned.
//!
//! `compile_ir` is the toolchain smoke path: textual IR through the same verifier,
//! optimization and object stages.

use std::collections::{BTreeMap, BTreeSet};

use hirsel_core_contract::{Disposition, Engine, Manifest, Passing};
use hirsel_lir::abi::abi_digest;
use hirsel_lir::abi_types::{
    CallableShape, FnAbi, ParameterPassing, PhysicalType, ScalarExtension,
};
use hirsel_lir::emission::{
    EmitInputs, IMMORTAL_PREFIX, IMMORTAL_PREFIX_BYTES, Manifested, OptLevel, PrefixWord,
    TOOLCHAIN_IMPORTS, artifact, callback_thunk_name, function_thunk_name, register_result,
    scalar_shape, site_code, slice_storage_with_index, symbol_name,
};
use hirsel_lir::ir::*;
use hirsel_lir::layout::{
    SEQUENCE_CAPACITY_OFFSET, align_up, payload_offset, sequence_elements_offset,
    sequence_slice_start, sequential_elements_offset,
};
use hirsel_lir::layout_types::{
    DropStep, Endian, FieldLayout, FinalizerRef, LayoutId, LayoutKind, LayoutRecord, LayoutTable,
    ObjectFormat, ScalarKind, ScanStep, SizeSpec, SizeTerm, TagForm, TargetSpec,
};
use hirsel_lir::object::ObjectFile;
use hirsel_lir::rt_program::{self, StepKind};
use hirsel_lir::runtime_abi::{
    CName, COUNT_IMMORTAL, COUNT_OFFSET, COUNT_UNIQUE, CTX_DUP_MAX_OFFSET, CType, DomainTag,
    LAYOUT_DESCRIPTOR_VERSION, LAYOUT_MAGIC, RuntimeEntry, small_pool,
};
use hirsel_lir::stage::{
    InputIdentity, ObjectArtifact, ObjectSymbol, ObjectSymbolKind, Ready, Relocation,
    Reproducibility, StageKind, ValidatedUnit,
};
use hirsel_syntax::{ContentHash, DefPath};

use crate::compare::{self, CompareError, ORDER_DEPTH, ORDER_EQUAL, ORDER_GREATER, ORDER_LESS};
use crate::context::{
    Access, Attribute, AttributeSlot, BasicBlock, BinOp, Builder, Cast, Context, Function,
    IntPredicate, Module, TargetMachine, Type, Value, loaded_version,
};
use crate::glue;
use crate::scalar;
use crate::{CodegenError, PINNED_LLVM, VerifyStage};
use hirsel_lir::debug::Profile;

mod ahead;
mod bindings;
mod blocks;
mod bounded;
mod callables;
mod calls;
mod constants;
mod construction;
mod descriptors;
mod flat;
mod flow;
mod foreign;
mod instructions;
mod ownership;
mod ownership_trace;
mod pair;
mod places;
mod program;
mod receipt;
mod resume;
mod sealed;
pub use program::emit_glue;
mod sequence_state;
mod soa;
mod structural;
use soa::ColumnRow;
mod signature;
mod values;

pub(crate) use pair::{PairField, PairWord, REGISTER_PAIR, pair_type};
use pair::{decode_word, encode_word, value_word};
use signature::{Aliasing, Hidden, Signature};

/// The pipeline LLVM runs between the two verifier passes.
pub(crate) fn pipeline(opt: OptLevel) -> &'static str {
    match opt {
        OptLevel::None => "default<O0>",
        OptLevel::Default => "default<O2>",
        OptLevel::Speed => "default<O3>",
    }
}

/// The pinned toolchain, as the object's reproducibility record names it.
fn toolchain() -> String {
    format!(
        "llvm {PINNED_LLVM}{}",
        if cfg!(feature = "native-tsan") {
            " tsan"
        } else {
            ""
        }
    )
}

/// Checks that the loaded libLLVM is the pin and that LLVM's DataLayout for `target` agrees
/// with every fact `TargetSpec` pins: the layout string, byte order, pointer width and each
/// scalar's size and ABI alignment. LLVM's answer is a cross-check, never adopted.
pub fn check_target(target: &TargetSpec) -> Result<(), CodegenError> {
    check_loaded()?;
    machine(target, false).map(drop)
}

fn check_loaded() -> Result<(), CodegenError> {
    let loaded = loaded_version();
    if loaded != PINNED_LLVM {
        return Err(CodegenError::LoadedLlvm { loaded });
    }
    Ok(())
}

fn machine(target: &TargetSpec, optimize: bool) -> Result<TargetMachine, CodegenError> {
    let machine =
        TargetMachine::new(target, optimize).map_err(|message| CodegenError::TargetMachine {
            triple: target.triple.clone(),
            message,
        })?;
    let facts = machine.data_layout();
    let mismatch = |what: &str, expected: String, actual: String| CodegenError::DataLayout {
        triple: target.triple.clone(),
        what: what.to_owned(),
        expected,
        actual,
    };
    if facts.text != target.llvm_data_layout {
        return Err(mismatch(
            "layout string",
            target.llvm_data_layout.clone(),
            facts.text,
        ));
    }
    let little = target.endian == Endian::Little;
    if facts.little_endian != little {
        return Err(mismatch(
            "byte order",
            format!("little endian {little}"),
            format!("little endian {}", facts.little_endian),
        ));
    }
    if facts.pointer_bytes != target.pointer_bytes {
        return Err(mismatch(
            "pointer bytes",
            target.pointer_bytes.to_string(),
            facts.pointer_bytes.to_string(),
        ));
    }
    let kinds = [
        ScalarKind::I8,
        ScalarKind::I16,
        ScalarKind::I32,
        ScalarKind::I64,
        ScalarKind::I128,
        ScalarKind::F32,
        ScalarKind::F64,
        ScalarKind::Pointer,
    ];
    for (kind, (size, align)) in kinds.iter().zip(facts.scalars) {
        let expected = (target.scalar_size(*kind), target.scalar_align(*kind));
        if (size, align) != expected {
            return Err(mismatch(
                &format!("{kind:?} size and alignment"),
                format!("{expected:?}"),
                format!("{:?}", (size, align)),
            ));
        }
    }
    Ok(machine)
}

/// Compiles textual LLVM IR to an object for `target`: the toolchain smoke path.
///
/// The IR may leave the triple and DataLayout unset; if it sets them they must be the
/// target's. The verifier runs before and after the optimization pipeline. Every LLVM handle
/// is dropped before the object is returned, so it owns all of its bytes.
pub fn compile_ir(
    ir: &str,
    target: &TargetSpec,
    opt: OptLevel,
) -> Result<ObjectFile, CodegenError> {
    check_loaded()?;
    let bytes = {
        let machine = machine(target, opt != OptLevel::None)?;
        let context = Context::new();
        let module = Module::parse(&context, "hirsel", ir)
            .map_err(|message| CodegenError::Parse { message })?;
        let triple = module.triple();
        if !triple.is_empty() && triple != target.triple {
            return Err(CodegenError::Triple {
                expected: target.triple.clone(),
                actual: triple,
            });
        }
        let layout = module.data_layout();
        if !layout.is_empty() && layout != target.llvm_data_layout {
            return Err(CodegenError::DataLayout {
                triple: target.triple.clone(),
                what: "module layout string".into(),
                expected: target.llvm_data_layout.clone(),
                actual: layout,
            });
        }
        module.set_target(&machine);
        optimize_and_emit(&module, &machine, opt, true, false)?
    };
    ObjectFile::read(target, bytes).map_err(|message| CodegenError::Object { message })
}

fn optimize_and_emit(
    module: &Module<'_>,
    machine: &TargetMachine,
    opt: OptLevel,
    verify: bool,
    retain_function_symbols: bool,
) -> Result<Vec<u8>, CodegenError> {
    optimize(module, machine, opt, verify)?;
    if retain_function_symbols {
        #[cfg(feature = "native-tsan")]
        module.bridge_thread_callbacks();
        module.retain_local_function_symbols();
    }
    module
        .emit_object(machine)
        .map_err(|message| CodegenError::Emit { message })
}

/// Whether unit emission runs LLVM's module verifier before and after its passes: with
/// the explicit `EmitInputs::verify_backend` harness policy and in builds with
/// debug assertions, never by default in the shipped
/// compiler: the verifier walks the whole module twice. This is cg_clif's policy
/// (rustc_codegen_cranelift src/lib.rs:257, `-Zverify-llvm-ir` or debug assertions or
/// its env switch; rustc's own `verify_llvm_ir` defaults off, rustc_session
/// src/options.rs:3014), which Cranelift emission shares (`hirsel_cranelift`'s
/// `enable_verifier`). Text IR (`compile_ir`) and the inspected IR texts are always
/// verified.
fn verify_backend(inputs: &EmitInputs<'_>) -> bool {
    cfg!(debug_assertions) || inputs.verify_backend
}

fn optimize(
    module: &Module<'_>,
    machine: &TargetMachine,
    opt: OptLevel,
    verify: bool,
) -> Result<(), CodegenError> {
    if verify {
        module.verify().map_err(|message| CodegenError::Verify {
            stage: VerifyStage::BeforeOptimization,
            message,
        })?;
    }
    if opt != OptLevel::None && module.has_unsigned_wrapping_sub() {
        module
            .run_passes(
                "function(sroa),cgscc(inline),function(sroa,early-cse,simplifycfg)",
                machine,
            )
            .map_err(|message| CodegenError::Passes { message })?;
        module.canonicalize_wrapping_masks();
    }
    module
        .run_passes(pipeline(opt), machine)
        .map_err(|message| CodegenError::Passes { message })?;
    crate::runtime::expand_fast_paths(module);
    module
        .run_passes("always-inline", machine)
        .map_err(|message| CodegenError::Passes { message })?;
    #[cfg(feature = "native-tsan")]
    module
        .instrument_thread(machine)
        .map_err(|message| CodegenError::Passes { message })?;
    if verify {
        module.verify().map_err(|message| CodegenError::Verify {
            stage: VerifyStage::AfterOptimization,
            message,
        })?;
    }
    Ok(())
}

/// Emits validated Ready units as one relocatable object for `inputs.target`.
pub fn emit_object(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    emit_object_profile(units, inputs, Profile::default())
}

/// Emits `units` as `emit_object` does, recording what `profile` asks for: each function's
/// debug intervals (`ObjectArtifact::debug`, for the link's debug object; the object
/// itself keeps no DWARF) and, under measurement with resolved sources, source-site call
/// scopes.
pub fn emit_object_profile(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    profile: Profile<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    emit_object_inner(units, inputs, profile, None)
}

/// Emit diagnostic shadow hooks for the supported leaf ownership subset.
/// Unsupported transfers are refused before any object is published.
pub fn emit_object_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    emit_object_inner(
        units,
        inputs,
        Profile::default(),
        Some(hirsel_lir::ownership_trace::native::Request::Leaf),
    )
}

fn emit_object_inner(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    profile: Profile<'_>,
    ownership: Option<hirsel_lir::ownership_trace::native::Request<'_>>,
) -> Result<ObjectArtifact, CodegenError> {
    // Ahead code also carries each resumable plan's state-0 alternate.
    if inputs.alternate
        && inputs.ahead.is_some_and(|plan| {
            !hirsel_lir::ahead::validate_cpu_workers(
                units,
                inputs.layouts,
                inputs.target,
                plan,
                inputs.fuel,
            )
        })
    {
        return Err(CodegenError::Object {
            message: "invalid CPU ahead certificate".into(),
        });
    }
    let resume = inputs.continuation.filter(|_| inputs.alternate);
    let (bytes, manifested, plan) = lower(
        units,
        inputs,
        profile,
        ownership,
        resume,
        |module, machine| {
            optimize_and_emit(
                module,
                machine,
                inputs.opt,
                verify_backend(inputs),
                inputs.alternate,
            )
        },
    )?;
    let (bytes, debug) = match plan {
        Some(plan) => crate::debug::split(&bytes, &plan)
            .map_err(|message| CodegenError::Object { message })?,
        None => (bytes, Vec::new()),
    };
    let file = ObjectFile::read(inputs.target, bytes)
        .map_err(|message| CodegenError::Object { message })?;
    let mut artifact = artifact(file, manifested, inputs, &toolchain())
        .map_err(|problem| CodegenError::Manifest { problem })?;
    if profile.debug || (inputs.measurement && profile.sources.is_some()) {
        artifact.reproducibility.options = ContentHash::of_parts(
            "hirsel.profile-lines-options.v2",
            [
                artifact.reproducibility.options.0.as_slice(),
                &[u8::from(profile.debug)],
            ],
        );
    }
    if inputs.measurement
        && let Some(sources) = profile.sources
    {
        artifact.reproducibility.options = ContentHash::of_parts(
            "hirsel.resolved-profile-options.v1",
            [
                artifact.reproducibility.options.0.as_slice(),
                hirsel_lir::profile::dependency_digest(sources).0.as_slice(),
            ],
        );
    }
    if ownership.is_some() {
        artifact.reproducibility.options = ContentHash::of_parts(
            "hirsel.native-ownership-leaf-options.v1",
            [
                artifact.reproducibility.options.0.as_slice(),
                hirsel_lir::ownership_trace::native::ABI_VERSION
                    .to_le_bytes()
                    .as_slice(),
            ],
        );
    }
    if let Some(fingerprint) = ownership.and_then(|request| request.fingerprint()) {
        artifact.reproducibility.options = ContentHash::of_parts(
            "hirsel.native-ownership-relocated-options.v1",
            [
                artifact.reproducibility.options.0.as_slice(),
                fingerprint.0.as_slice(),
            ],
        );
    }
    artifact.debug = debug;
    Ok(artifact)
}

/// Every resumable plan's state alternates and dispatcher (`resume`), as one object.
pub fn emit_resume_object(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &hirsel_lir::continuation::ContinuationPlans,
) -> Result<ObjectArtifact, CodegenError> {
    hirsel_lir::continuation::validate_runtime_operations(
        units,
        inputs.layouts,
        inputs.target,
        plans,
    )
    .map_err(|error| CodegenError::Invalid {
        definition: units
            .first()
            .expect("entered runtime units")
            .unit()
            .header
            .definition
            .clone(),
        what: error.to_string(),
    })?;
    let (bytes, manifested, _) = lower(
        units,
        inputs,
        Profile::default(),
        None,
        Some(plans),
        |module, machine| {
            optimize_and_emit(module, machine, inputs.opt, verify_backend(inputs), false)
        },
    )?;
    let file = ObjectFile::read(inputs.target, bytes)
        .map_err(|message| CodegenError::Object { message })?;
    artifact(file, manifested, inputs, &toolchain())
        .map_err(|problem| CodegenError::Manifest { problem })
}

/// The verified, optimized LLVM IR `emit_object` would emit as an object: the text
/// codegen controls inspect.
pub fn emit_optimized_ir(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    lower(
        units,
        inputs,
        Profile::default(),
        None,
        None,
        |module, machine| {
            optimize(module, machine, inputs.opt, true)?;
            Ok(module.text())
        },
    )
    .map(|(text, ..)| text)
}

/// The verified module text before optimization, for IR-level controls.
pub fn emit_ir(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    lower(
        units,
        inputs,
        Profile::default(),
        None,
        None,
        |module, _| Ok(module.text()),
    )
    .map(|(text, ..)| text)
}

/// The verified preoptimization module with explicit ownership shadow hooks.
pub fn emit_ir_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    lower(
        units,
        inputs,
        Profile::default(),
        Some(hirsel_lir::ownership_trace::native::Request::Leaf),
        None,
        |module, _| Ok(module.text()),
    )
    .map(|(text, ..)| text)
}

/// Emit exactly the plans preserved by earlier compiler passes. No leaf plan
/// fallback can supply a missing or stale unit in this diagnostic request.
pub fn emit_object_relocated_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &BTreeMap<ContentHash, hirsel_lir::ownership_trace::Plan>,
) -> Result<ObjectArtifact, CodegenError> {
    emit_object_inner(
        units,
        inputs,
        Profile::default(),
        Some(hirsel_lir::ownership_trace::native::Request::Relocated(
            plans,
        )),
    )
}

/// Emit the explicit resource selection of a source diagnostic request.
pub fn emit_object_resources_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &BTreeMap<ContentHash, hirsel_lir::ownership_trace::Plan>,
) -> Result<ObjectArtifact, CodegenError> {
    emit_object_inner(
        units,
        inputs,
        Profile::default(),
        Some(hirsel_lir::ownership_trace::native::Request::Resources(
            plans,
        )),
    )
}

pub fn emit_ir_relocated_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &BTreeMap<ContentHash, hirsel_lir::ownership_trace::Plan>,
) -> Result<String, CodegenError> {
    lower(
        units,
        inputs,
        Profile::default(),
        Some(hirsel_lir::ownership_trace::native::Request::Relocated(
            plans,
        )),
        None,
        |module, _| Ok(module.text()),
    )
    .map(|(text, ..)| text)
}

/// Lowers `units` into one module for the target and hands it to `finish`.
fn lower<T>(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    profile: Profile<'_>,
    ownership: Option<hirsel_lir::ownership_trace::native::Request<'_>>,
    resume: Option<&hirsel_lir::continuation::ContinuationPlans>,
    finish: impl FnOnce(&Module<'_>, &TargetMachine) -> Result<T, CodegenError>,
) -> Result<(T, Manifested, Option<crate::debug::DebugPlan>), CodegenError> {
    if inputs
        .ahead
        .is_some_and(|plan| !hirsel_lir::ahead::validate_cpu_summaries(units, inputs.layouts, plan))
    {
        return Err(CodegenError::Object {
            message: "invalid CPU result summary recipe".into(),
        });
    }
    check_loaded()?;
    let ownership_plans = if let Some(ownership) = ownership {
        if inputs.alternate || !inputs.available_externally.is_empty() {
            return Err(CodegenError::Unsupported {
                definition: units
                    .first()
                    .map(|u| u.unit().header.definition.clone())
                    .ok_or_else(|| CodegenError::Glue {
                        what: "ownership emission has no units".into(),
                    })?,
                what: "ownership tracing with alternate/template code".into(),
            });
        }
        Some(
            ownership
                .prepare(units, inputs.layouts)
                .map_err(|what| CodegenError::Glue { what })?,
        )
    } else {
        None
    };
    let owned;
    let index = match inputs.index.filter(|index| index.matches(inputs.layouts)) {
        Some(index) => index,
        None => {
            owned = hirsel_lir::LayoutIndex::new(inputs.layouts);
            &owned
        }
    };
    let inputs = &EmitInputs {
        index: Some(index),
        ..inputs.clone()
    };
    for unit in units {
        hirsel_lir::emission::check_receipt(unit, inputs).map_err(|problem| {
            CodegenError::Receipt {
                definition: unit.unit().header.definition.clone(),
                problem,
            }
        })?;
    }
    let mut manifested = Manifested::default();
    let mut optimizer_only = BTreeSet::new();
    let finished = {
        let machine = machine(inputs.target, inputs.opt != OptLevel::None)?;
        if inputs.alternate {
            machine.stack_sizes();
        }
        let context = Context::new();
        let module = Module::new(&context, "hirsel");
        module.set_target(&machine);
        let debug = profile
            .debug
            .then(|| crate::context::DebugInfo::new(&module, inputs.opt != OptLevel::None));
        let mut plan = crate::debug::DebugPlan::default();
        let mut functions = Vec::new();
        for unit in units {
            // An ahead object carries both alternates; a resume object only states.
            if resume.is_some() && !inputs.alternate {
                break;
            }
            let unit = unit.unit();
            let template = !inputs.alternate
                && (unit.header.origin == UnitOrigin::Library
                    || inputs.inline_instances.contains(&unit.header.instance));
            let copy = inputs.available_externally.contains(&unit.header.instance);
            let abi = fn_abi(inputs.layouts, unit.header.abi, &unit.header.definition)?;
            let name = if inputs.alternate {
                hirsel_lir::ahead::alternate_name(&unit.header.definition, unit.header.instance)
            } else {
                symbol_name(&unit.header.definition, Some(&unit.header.instance))
            };
            if module.defines(&name)
                || manifested.exports.contains_key(&name)
                || manifested.library.contains(&name)
            {
                return Err(CodegenError::DuplicateSymbol { name });
            }
            let external = Signature::of(&module, inputs.layouts, abi, &unit.header.definition)?;
            // These checked sequence bodies inline into both their adapter and
            // callers. Keep the ordinary root companions on that private path
            // too, so length and uniqueness survive each loop iteration.
            let private_roots = inputs.alternate
                && (unit.header.definition.is_core("list.push")
                    || unit.header.definition.is_core("list.reserve")
                    || unit.header.definition.is_core("$native.list_push_spare"));
            let signature = if unit.header.origin == UnitOrigin::Library
                && (!inputs.alternate || private_roots)
            {
                Signature::of(&module, inputs.layouts, abi, &unit.header.definition)?
                    .with_roots(&module, inputs.layouts)
            } else {
                Signature::of(&module, inputs.layouts, abi, &unit.header.definition)?
            };
            let body_name = if signature.roots.is_empty() {
                name.clone()
            } else {
                format!("{name}{ROOTS_SUFFIX}")
            };
            let function = signature.declare(
                &module,
                inputs.layouts,
                &body_name,
                &unit.header.definition,
                true,
                Aliasing::of(inputs.callbacks),
            )?;
            if inputs.alternate {
                // These helpers have the same checked storage handoffs as
                // their caller. Inlining preserves every save boundary and
                // lets the mapper keep its sequence fields in registers.
                let attribute = if private_roots {
                    Attribute::AlwaysInline
                } else {
                    Attribute::NoInline
                };
                module.add_attribute(function, AttributeSlot::Function, attribute);
                module.add_attribute(function, AttributeSlot::Function, Attribute::NoRedZone);
                module.string_attribute(function, "disable-tail-calls", "true");
                module.string_attribute(function, "frame-pointer", "all");
            }
            let entry = if signature.roots.is_empty() {
                function
            } else {
                let entry = external.declare(
                    &module,
                    inputs.layouts,
                    &name,
                    &unit.header.definition,
                    true,
                    Aliasing::of(inputs.callbacks),
                )?;
                define_root_adapter(
                    &module,
                    entry,
                    function,
                    &signature,
                    &external,
                    inputs.layouts,
                );
                if inputs.alternate {
                    module.add_attribute(entry, AttributeSlot::Function, Attribute::NoRedZone);
                    module.string_attribute(entry, "disable-tail-calls", "true");
                    module.string_attribute(entry, "frame-pointer", "all");
                }
                if copy {
                    module.make_available_externally(entry);
                    optimizer_only.insert(name.clone());
                    optimizer_only.insert(body_name.clone());
                } else {
                    module.make_weak_odr(entry);
                }
                module.add_attribute(entry, AttributeSlot::Function, Attribute::InlineHint);
                manifested.library.insert(body_name);
                // The adapter has its unit's scope, so a body inlined into it keeps its
                // locations.
                if let Some(debug) = debug.as_ref() {
                    let site = unit.tables.source_anchors
                        [unit.blocks[unit.entry.index()].anchor.index()]
                    .site;
                    let column = plan.column(unit);
                    let line = hirsel_lir::debug::SiteTable::new(unit)
                        .code(site)
                        .unwrap_or(0);
                    let scope = debug.function(entry, &unit.header.definition.to_string(), line);
                    debug.locate_all(entry, scope, line, column);
                    if !inputs.available_externally.contains(&unit.header.instance) {
                        plan.functions.push((name.clone(), column - 1, site));
                    }
                }
                entry
            };
            // A small source unit another object implements and exports: here only an
            // inlinable body. Calls the optimizer keeps stay imports of that export.
            if copy && !template {
                module.make_available_externally(function);
                // Remove aggregate call traffic without forcing loop bodies past
                // LLVM's cost model. Copies already have a bounded source size.
                let attribute = if abi
                    .result
                    .as_ref()
                    .is_some_and(|result| result.register.is_none())
                    && {
                        let successors: Vec<_> = unit
                            .terminators
                            .iter()
                            .map(|terminator| {
                                terminator
                                    .normal_edges(&unit.pools)
                                    .iter()
                                    .map(|edge| unit.tables.edges[edge.index()].target.index())
                                    .collect()
                            })
                            .collect();
                        hirsel_graph::feedback_edges(&successors, unit.entry.index()).is_empty()
                    } {
                    Attribute::AlwaysInline
                } else {
                    Attribute::InlineHint
                };
                module.add_attribute(function, AttributeSlot::Function, attribute);
                functions.push((unit, function, signature));
                continue;
            }
            // rustc_middle/src/mono.rs:132: shared owners at O0, local optimizer
            // copies above. A template's owner retains its definition independently
            // of its callers; an available_externally copy emits no machine code.
            if template {
                if copy {
                    module.make_available_externally(function);
                    optimizer_only.insert(name.clone());
                } else {
                    module.make_weak_odr(function);
                }
                let inline_map = ["map.get", "map.contains", "map.insert", "map.upsert_with"]
                    .iter()
                    .any(|name| unit.header.definition.is_core(name));
                let attribute = if inline_map
                    || unit.header.definition.is_core("list.push")
                    || unit.header.definition.is_core("$native.list_push_spare")
                    || unit.header.definition.is_core("net.read_into")
                    || unit.header.definition.is_core("read_half.read_into")
                {
                    Attribute::AlwaysInline
                } else {
                    Attribute::InlineHint
                };
                module.add_attribute(function, AttributeSlot::Function, attribute);
                if unit.header.definition.is_core("$native.list_grow") {
                    module.add_attribute(function, AttributeSlot::Function, Attribute::Cold);
                    module.add_attribute(function, AttributeSlot::Function, Attribute::NoInline);
                }
                // A finalizer's entry is also called through a descriptor in
                // the separate glue object. Retain it even if every local call
                // inlines away. Its own receiver layout proves this dependency,
                // so retention reads no unrelated program layouts or numbering.
                if abi.parameters.first().is_some_and(|parameter| {
                    inputs
                        .layouts
                        .layout(parameter.layout)
                        .is_some_and(|layout| {
                            layout.finalizer.as_ref().is_some_and(|finalizer| {
                                finalizer.definition == unit.header.definition
                            })
                        })
                }) {
                    manifested.kept.insert(name.clone());
                }
                manifested.library.insert(name);
                if abi.shape == CallableShape::Definition {
                    // A library operation can be a FunctionRef (integer Order
                    // dictionaries). Its callable thunk has the same linkage
                    // as its body: retained by its owner, optimizer-only in callers.
                    let name =
                        function_thunk_name(&unit.header.definition, Some(&unit.header.instance));
                    let thunk = define_function_thunk(&module, &name, entry, &external);
                    if copy {
                        module.make_available_externally(thunk);
                        optimizer_only.insert(name.clone());
                    } else {
                        module.make_weak_odr(thunk);
                    }
                    module.add_attribute(thunk, AttributeSlot::Function, Attribute::InlineHint);
                    manifested.library.insert(name);
                }
                functions.push((unit, function, signature));
                continue;
            }
            manifested.exports.insert(
                name.clone(),
                ObjectSymbol {
                    name,
                    kind: ObjectSymbolKind::Function,
                    definition: Some(unit.header.definition.clone()),
                    abi: Some(abi_digest(abi)),
                },
            );
            if !inputs.alternate && abi.shape == CallableShape::Definition {
                let thunk =
                    function_thunk_name(&unit.header.definition, Some(&unit.header.instance));
                define_function_thunk(&module, &thunk, function, &signature);
                manifested.exports.insert(
                    thunk.clone(),
                    ObjectSymbol {
                        name: thunk,
                        kind: ObjectSymbolKind::Function,
                        definition: Some(unit.header.definition.clone()),
                        abi: Some(abi_digest(abi)),
                    },
                );
            }
            functions.push((unit, function, signature));
        }
        let resumed = match resume {
            Some(plans) => resume::declare(&module, inputs, units, plans, &mut manifested)?,
            None => Vec::new(),
        };
        for (unit, plan, state, function) in resumed {
            let abi = fn_abi(inputs.layouts, unit.header.abi, &unit.header.definition)?;
            let signature = Signature::of(&module, inputs.layouts, abi, &unit.header.definition)?;
            let frame = module.parameter(function, signature.types.len() as u32);
            let mut emitter =
                FnEmitter::new(&module, inputs, unit, function, signature, &mut manifested);
            emitter.resume = Some(resume::Resume {
                plan,
                state,
                frame,
                resuming: None,
            });
            emitter.emit()?;
        }
        let depth_leaf = hirsel_lir::invocations::room::depth_leaf_units(
            units.iter().map(|u| u.unit()),
            inputs.layouts,
        );
        let counter_units =
            hirsel_lir::invocations::room::counter_units(units.iter().map(|u| u.unit()));
        for (unit, function, signature) in functions {
            let rooted = !signature.roots.is_empty();
            let mut emitter =
                FnEmitter::new(&module, inputs, unit, function, signature, &mut manifested);
            emitter.room = hirsel_lir::invocations::room::RoomChecks::of(unit, &depth_leaf);
            // A continuation re-enters with its callee's cleanup mode still
            // live. Its saved admission needs the runtime's retained token.
            if !inputs.alternate {
                emitter.counters =
                    hirsel_lir::invocations::room::RoomChecks::counters_of(unit, &counter_units);
            }
            emitter.ownership = ownership_plans
                .as_ref()
                .and_then(|plans| plans.get(&unit.header.instance));
            if let Some(debug) = debug.as_ref() {
                let site =
                    unit.tables.source_anchors[unit.blocks[unit.entry.index()].anchor.index()].site;
                let column = plan.column(unit);
                // Content-only locations: a complete site's table code as the line.
                let line = hirsel_lir::debug::SiteTable::new(unit)
                    .code(site)
                    .unwrap_or(0);
                let scope = debug.function(function, &unit.header.definition.to_string(), line);
                if !inputs.available_externally.contains(&unit.header.instance) {
                    // The scope is the body's: a library unit's behind its root adapter.
                    let name = symbol_name(&unit.header.definition, Some(&unit.header.instance));
                    let name = if rooted {
                        format!("{name}{ROOTS_SUFFIX}")
                    } else {
                        name
                    };
                    plan.functions.push((name, column - 1, site));
                }
                emitter.debug_sites = Some(hirsel_lir::debug::SiteTable::new(unit));
                emitter.debug = Some((debug, scope, column));
                debug.location(&emitter.prologue, scope, line, column);
                debug.location(&emitter.builder, scope, line, column);
            }
            if inputs.measurement {
                emitter.measured = profile.sources.and_then(|s| s.get(&unit.header.definition));
            }
            emitter.emit()?;
        }
        // A library finalizer or C callback entry is kept for glue to call.
        let kept: Vec<_> = manifested
            .kept
            .iter()
            .filter(|name| module.defines(name) && !optimizer_only.contains(*name))
            .map(|name| module.function_address(name))
            .collect();
        module.keep(&kept);
        // Copied root bodies were visible during lowering for the private root ABI.
        // They are optimizer-only: residual calls resolve to the template's owner.
        for name in &optimizer_only {
            manifested.library.remove(name);
            manifested
                .imports
                .entry(name.clone())
                .or_insert_with(|| ObjectSymbol {
                    name: name.clone(),
                    kind: ObjectSymbolKind::Function,
                    definition: None,
                    abi: None,
                });
        }
        // The retained owner symbols are visible to other objects' link manifests.
        // `library` also tracks the private root ABI while lowering, so promote only
        // after every body has been emitted.
        for unit in units {
            let header = &unit.unit().header;
            if inputs.available_externally.contains(&header.instance) {
                continue;
            }
            let name = symbol_name(&header.definition, Some(&header.instance));
            let abi = fn_abi(inputs.layouts, header.abi, &header.definition)?;
            for name in [
                format!("{name}{ROOTS_SUFFIX}"),
                name,
                function_thunk_name(&header.definition, Some(&header.instance)),
                callback_thunk_name(&header.definition, Some(&header.instance)),
            ] {
                if manifested.library.remove(&name) {
                    manifested.exports.insert(
                        name.clone(),
                        ObjectSymbol {
                            name,
                            kind: ObjectSymbolKind::Function,
                            definition: Some(header.definition.clone()),
                            abi: Some(abi_digest(abi)),
                        },
                    );
                }
            }
        }
        // An import the module also defines is internal to the object.
        manifested.imports.retain(|name, _| {
            !manifested.exports.contains_key(name) && !manifested.library.contains(name)
        });
        if let Some(debug) = &debug {
            debug.finalize();
        }
        #[cfg(feature = "native-tsan")]
        {
            crate::tsan::manifest(&mut manifested);
            if inputs.alternate {
                crate::tsan::manifest_bridges(&mut manifested);
            }
        }
        (finish(&module, &machine)?, profile.debug.then_some(plan))
    };
    let (finished, plan) = finished;
    Ok((finished, manifested, plan))
}

/// Defines `name`, the function-reference thunk of the definition entry `target`: the
/// callable ABI of the same declared type (an environment after the context), forwarding
/// every other argument and what the entry returns.
fn define_function_thunk<'m>(
    module: &'m Module<'m>,
    name: &str,
    target: Function<'m>,
    signature: &Signature<'m, '_>,
) -> Function<'m> {
    let count = signature.types.len();
    let mut types = signature.types.clone();
    types.insert(1, module.ptr_type());
    if !signature.abi.caller_site {
        types.splice(2..2, [module.int_type(64), module.int_type(64)]);
    }
    let ty = module.function_type(signature.returns, &types);
    let thunk = module
        .function(name, ty)
        .expect("a thunk name is never declared with another type");
    if signature.returns_status() {
        module.add_attribute(thunk, AttributeSlot::Return, Attribute::ZeroExt);
    }
    let builder = Builder::new(module);
    builder.position(module.append_block(thunk));
    let mut arguments = vec![module.parameter(thunk, 0)];
    let inserted = if signature.abi.caller_site { 1 } else { 3 };
    arguments.extend((1..count as u32).map(|i| module.parameter(thunk, i + inserted)));
    let status = builder.call(target, &arguments);
    signature.attribute_call(module, status);
    builder.ret(status);
    thunk
}

/// The public address ABI is kept at function-reference and external boundaries.
/// Direct library calls enter the root-value body instead.
fn define_root_adapter<'m>(
    module: &'m Module<'m>,
    entry: Function<'m>,
    target: Function<'m>,
    signature: &Signature<'m, '_>,
    external: &Signature<'m, '_>,
    layouts: &LayoutTable,
) {
    let builder = Builder::new(module);
    builder.position(module.append_block(entry));
    let original_arity = signature
        .companions
        .iter()
        .flatten()
        .copied()
        .min()
        .unwrap_or(signature.types.len() as u32);
    let mut args: Vec<_> = (0..original_arity)
        .map(|i| module.parameter(entry, i))
        .collect();
    for &i in &signature.roots {
        let first = signature.first[i] as usize;
        args[first] = builder.load(module.ptr_type(), args[first], 8);
    }
    for (field, &i) in signature.roots.iter().enumerate() {
        if signature.companions[field].is_some() {
            let pointer = args[signature.first[i] as usize];
            let data = builder.offset(
                pointer,
                module.const_int(
                    module.int_type(64),
                    u128::from({
                        let LayoutKind::Block { payload } = layouts
                            .layout(signature.abi.parameters[i].layout)
                            .expect("root layout")
                            .kind
                        else {
                            unreachable!("sequence block")
                        };
                        payload_offset(layouts.layout(payload).expect("sequence payload").align)
                            .expect("validated alignment")
                    }),
                ),
            );
            let length = builder.load_access(module.int_type(64), data, 8, Access::Length);
            // Not deferred: the body stores the count it sets, as the external caller reads it.
            args.extend([
                length,
                module.const_zero(module.int_type(1)),
                module.const_zero(module.int_type(1)),
                module.const_zero(module.int_type(1)),
            ]);
        }
    }
    let returned = builder.call(target, &args);
    signature.attribute_call(module, returned);
    for (field, &i) in signature.roots.iter().enumerate() {
        builder.store(
            signature.root_pointer(&builder, returned, field),
            module.parameter(entry, signature.first[i]),
            8,
        );
    }
    let status = signature.status(&builder, returned);
    let value = signature.result(&builder, returned);
    builder.ret(external.returned(&builder, module.const_zero(external.returns), status, value));
}

fn fn_abi<'a>(
    layouts: &'a LayoutTable,
    id: hirsel_lir::FnAbiId,
    definition: &DefPath,
) -> Result<&'a FnAbi, CodegenError> {
    let abi = layouts
        .fn_abis
        .get(id.0 as usize)
        .ok_or_else(|| CodegenError::Invalid {
            definition: definition.clone(),
            what: format!("no FnAbi {}", id.0),
        })?;
    hirsel_lir::abi::check_implemented_transport(abi).map_err(|what| CodegenError::Invalid {
        definition: definition.clone(),
        what: what.into(),
    })?;
    Ok(abi)
}

/// Bytes of a `hirsel_rt_v2_region` record (`HIRSEL_RT_V2_ASSERT_SIZE` in the header).

/// The name suffix of a library body's private root-value entry. It changes with that
/// entry's signature, so a `linkonce_odr` body of an older signature never merges
/// with a caller of this one.
const ROOTS_SUFFIX: &str = ".roots9";

/// The private descriptor of the empty environment a function reference carries.

/// How a value is held in SSA form: unit, one scalar, or the bytes of a fixed-size inline
/// value (aggregates, variants, callables, handles), moved whole and read through memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Unit,
    Scalar(ScalarKind),
    /// Keep code and environment as pointers through SSA, without byte packing.
    Callable,
    /// An LLVM array of the widest integer that divides both `size` and `align`, so an
    /// aligned pointer or word field stays one store when SROA splits the value.
    Bytes {
        size: u64,
        align: u32,
    },
}

/// One field's FlatFormatV0 bytes in a flat construction: a fixed field's value, or an
/// encoded source's `length` bytes at `bytes`, written behind a u64 length prefix.
#[derive(Clone, Copy)]
enum FlatPart<'m> {
    Fixed {
        value: ValueId,
        size: u64,
    },
    Encoded {
        bytes: Value<'m>,
        length: Value<'m>,
    },
    Sequence {
        source: Place<'m>,
        encoding: LayoutId,
    },
    /// A flat list builder's payload (LIR schema revision 22): its count and
    /// used bytes are the list's count and content, copied whole.
    Built {
        data: Value<'m>,
        first: u64,
    },
    /// A byte field an open element built in place (LIR schema revision 24):
    /// its length word and `length` bytes are already written.
    InPlace {
        length: Value<'m>,
    },
}

/// A prepared address: its pointer, layout and the alignment it is known to have.
#[derive(Clone, Copy, Debug)]
struct Place<'m> {
    columns: Option<ColumnRow<'m>>,
    ptr: Value<'m>,
    layout: LayoutId,
    align: u32,
    /// The counted block (its header) whose payload holds this place, if any.
    block: Option<Value<'m>>,
}

pub(crate) fn low_bit(n: u64) -> u32 {
    if n == 0 {
        u32::MAX
    } else {
        1u32 << n.trailing_zeros().min(31)
    }
}

/// The u32 variant ordinal at `offset` bytes from `base` (aligned to `align`), as an `i32`.
/// Every tag read in this crate reaches it, the comparator's included.
pub(crate) fn load_tag<'m>(
    builder: &Builder<'m>,
    base: Value<'m>,
    align: u32,
    offset: u64,
    form: TagForm,
    unaligned: bool,
) -> Value<'m> {
    let module = builder.module();
    let i64_ty = module.int_type(64);
    let i32_ty = module.int_type(32);
    let at = if offset == 0 {
        base
    } else {
        builder.offset(base, module.const_int(i64_ty, u128::from(offset)))
    };
    let align = |width: u32| {
        if unaligned {
            1
        } else {
            align.min(low_bit(offset)).min(width)
        }
    };
    match form {
        TagForm::Ordinal => builder.load(i32_ty, at, align(4)),
        TagForm::LiveBit { bit } => {
            let word = builder.load(i64_ty, at, align(8));
            let set = builder.binary(BinOp::And, word, module.const_int(i64_ty, 1u128 << bit));
            let live = builder.icmp(IntPredicate::Ne, set, module.const_zero(i64_ty));
            builder.select(
                live,
                module.const_int(i32_ty, 1),
                module.const_int(i32_ty, 0),
            )
        }
        // A null-pointer niche: the dataful variant's block reference word is non-null, the
        // fieldless variant's null.
        TagForm::Niche { nullary, dataful } => {
            let ptr_ty = module.ptr_type();
            let word = builder.load(ptr_ty, at, align(8));
            let null = builder.icmp(IntPredicate::Eq, word, module.const_zero(ptr_ty));
            builder.select(
                null,
                module.const_int(i32_ty, u128::from(nullary)),
                module.const_int(i32_ty, u128::from(dataful)),
            )
        }
    }
}

/// Companion storage is private and promotable just like the root itself.
/// `unique` is a must fact: false asks for a real count test, never claims sharedness.
#[derive(Clone, Copy)]
struct SequenceState<'m> {
    /// Must-unique: the slot's block has one holder, so it is writable and no tracked
    /// arena block.
    unique: Value<'m>,
    /// Must-untracked: the slot's block is no tracked arena block (MEM-6), so a push needs
    /// no charge. Growth and a detach keep it; only a new block in the slot loses it.
    untracked: Value<'m>,
    length: Value<'m>,
    /// The slot holds a live block whose count word may lag `length`: a
    /// `SeqSetLen` here, or a private root call or entry, deferred its store.
    /// A flush stores it (only where it differs, so a static block is never
    /// written) before anything reads the count from memory.
    dirty: Value<'m>,
}

/// Emits one Ready unit into its declared function.
struct FnEmitter<'m, 'a> {
    module: &'m Module<'m>,
    inputs: &'a EmitInputs<'a>,
    unit: &'a LirUnit<Ready>,
    function: Function<'m>,
    signature: Signature<'m, 'a>,
    manifested: &'a mut Manifested,
    builder: Builder<'m>,
    /// With debug intervals: the function's scope and its item's column (`crate::debug`).
    debug_sites: Option<hirsel_lir::debug::SiteTable>,
    debug: Option<(
        &'a crate::context::DebugInfo<'m>,
        crate::context::DebugScope,
        u32,
    )>,
    /// Under measurement: the resolved source its instrumentation embeds.
    measured: Option<&'a crate::ProfileSource>,
    /// Positioned at the end of the prologue, which holds every stack slot.
    prologue: Builder<'m>,
    prologue_block: BasicBlock<'m>,
    context: Value<'m>,
    values: Vec<Option<Value<'m>>>,
    /// Storage version names: never operands, so they have no LLVM value.
    versions: BTreeSet<ValueId>,
    slots: Vec<Option<Place<'m>>>,
    sequence_states: Vec<Option<SequenceState<'m>>>,
    read_roots: Vec<Option<SlotId>>,
    preserved_lengths: Vec<Option<Value<'m>>>,
    /// Slots whose count store can be deferred (`SequenceState::dirty`): a
    /// sequence state's slot that a `SeqSetLen`, a private root call or the
    /// entry (a root parameter's companions) can leave ahead of its block.
    deferred: Vec<bool>,
    /// Deferred slots this unit reads whole without taking them (a borrow or a
    /// bitwise read): such a view, whose length is not preserved, may read the
    /// slot's block from memory.
    viewed: Vec<bool>,
    /// Slots of root parameters that hand their length back to the caller.
    companion_slots: Vec<bool>,
    /// Slots that always hold one static's pointer-free bytes, addressed in place in
    /// read-only data (`hirsel_lir::statics::constant_slots`).
    constant_slots: Vec<Option<hirsel_lir::statics::ConstantSlot<'a>>>,
    /// A root parameter slot's `defer` companion: false when its caller keeps the
    /// count in the block, so a `SeqSetLen` stores it and the return flushes.
    defer_params: Vec<Option<Value<'m>>>,
    fresh_detaches: BTreeSet<ValueId>,
    /// Whether a detach's copy is no tracked arena block (MEM-6), computed at the detach.
    detach_untracked: BTreeMap<ValueId, Value<'m>>,
    /// The preserved length of the view an address is projected from, for its
    /// first step: an index into it bounds by that length, not the count word.
    known_count: Option<Value<'m>>,
    /// The instruction being emitted: its block and index.
    at: (BlockId, usize),
    /// A builder's release whose block a flat construction may have taken
    /// over, and whether it did (`adopt_builder`).
    adopted: BTreeMap<ValueId, Value<'m>>,
    addresses: Vec<Option<Place<'m>>>,
    /// Per address of a variable flat element, where its sequence's elements begin
    /// (`FlatNext` offsets count from there; LIR schema revision 7).
    flat_contents: Vec<Option<Value<'m>>>,
    /// Set by the last variable flat element step `project` took.
    flat_content: Option<Value<'m>>,
    blocks: Vec<BasicBlock<'m>>,
    /// Per block, a phi per parameter (None for version joins).
    phis: Vec<Vec<Option<Value<'m>>>>,
    failures: BTreeMap<FailureId, BasicBlock<'m>>,
    /// The environment block a closure body was entered with.
    environment: Option<Value<'m>>,
    /// Per compiler-local region instance, its `hirsel_rt_v2_region` record in the frame.
    regions: Vec<Value<'m>>,
    /// A register-result entry's one return block and its status and word phis.
    register_return: Option<(BasicBlock<'m>, Value<'m>, Value<'m>)>,
    /// The `i8` every inline drop accumulates its status into: one per function,
    /// zeroed as each drop starts, so promoting it is one pass over the function
    /// rather than one per drop.
    drop_status: Option<Value<'m>>,
    /// Per block, whether it charges fuel (`hirsel_lir::fuel`); empty without `inputs.fuel`.
    fuel_points: Vec<bool>,
    ahead_state: Option<Value<'m>>,
    ahead_counter: Option<Value<'m>>,
    ownership: Option<&'a hirsel_lir::ownership_trace::native::LeafPlan>,
    ownership_frame: Option<Value<'m>>,
    ownership_cleanup: Option<hirsel_lir::ownership_trace::native::NativeMarker>,
    /// A resumable state alternate (`resume`).
    resume: Option<resume::Resume<'a, 'm>>,
    /// Admissions of depth-leaf callees, lowered to room checks with no retirement.
    room: hirsel_lir::invocations::room::RoomChecks,
    /// The prologue's room test, built at the first room check (`no_room`).
    no_room: Option<Value<'m>>,
    counters: hirsel_lir::invocations::room::RoomChecks,
    counter_cell: Option<(Value<'m>, Value<'m>)>,
}

type Emitted<T = ()> = Result<T, CodegenError>;

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::{ItemPath, PackageId};

    fn definition(name: &str) -> DefPath {
        DefPath(ItemPath {
            package: PackageId {
                name: "fixture".into(),
                version: "0.1".into(),
            },
            modules: vec!["m_n".into()],
            name: name.into(),
        })
    }

    /// Since `.roots5`, a returned root's pointer word is the pointer itself, for sequence
    /// roots too, whose must-unique fact has a word of its own. Under `.roots4` the
    /// sequence pointer carried that fact in its low bits, and every loop over a
    /// root call or inlined body masked and re-tagged it (`ptrtoint`, `and -4`,
    /// `inttoptr`) per iteration; a headerless payload's low bits are kept the same way.
    #[test]
    fn root_returns_carry_pointers_untagged() {
        use hirsel_lir::abi_types::{CallMultiplicity, CallableShape, FnAbiKey};
        let context = Context::new();
        let module = Module::new(&context, "root-pointers");
        let abi = FnAbi {
            receipts: None,
            nonretaining: Vec::new(),
            nonretaining_captures: Vec::new(),
            scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
            scoped_entries: Vec::new(),
            key: FnAbiKey {
                receipt_contract: None,
                callable: ContentHash::of_bytes(b"root"),
                abi_version: 1,
            },
            shape: CallableShape::Definition,
            multiplicity: CallMultiplicity::Many,
            context: true,
            environment: false,
            caller_site: false,
            destination: false,
            result: None,
            result_arena: None,
            diverges: false,
            parameters: vec![],
            version: 1,
        };
        let (ptr, i64_ty, i1) = (module.ptr_type(), module.int_type(64), module.int_type(1));
        for sequence in [false, true] {
            let mut signature = Signature::runtime(&module, &abi, &definition("root")).unwrap();
            signature.roots = vec![0];
            signature.sequence_roots = vec![sequence];
            if sequence {
                signature.uniques = vec![Some(2)];
                signature.status_field = 3;
                signature.returns = module.struct_type(&[ptr, i64_ty, i1, i64_ty]);
            } else {
                signature.uniques = vec![None];
                signature.returns = module.struct_type(&[ptr, i64_ty]);
            }
            let builder = Builder::new(&module);
            let result_ty = module.struct_type(&[ptr, i1, module.int_type(8)]);
            let function = module
                .function(
                    &format!("roots{}", u8::from(sequence)),
                    module.function_type(result_ty, &[ptr, i1, i64_ty]),
                )
                .unwrap();
            builder.position(module.append_block(function));
            let mut returned = module.const_zero(signature.returns);
            returned = builder.insert(returned, module.parameter(function, 0), 0);
            returned = builder.insert(
                returned,
                module.parameter(function, 2),
                signature.status_field,
            );
            let unique = if sequence {
                returned = builder.insert(returned, module.parameter(function, 1), 2);
                signature.root_unique(&builder, returned, 0)
            } else {
                module.const_zero(i1)
            };
            let pointer = signature.root_pointer(&builder, returned, 0);
            let status = signature.status(&builder, returned);
            let mut result = builder.insert(module.const_zero(result_ty), pointer, 0);
            result = builder.insert(result, unique, 1);
            builder.ret(builder.insert(result, status, 2));
        }
        module.verify().unwrap();
        let text = module.text();
        for cast in ["ptrtoint", "inttoptr", " and ", " or ", " lshr "] {
            assert!(!text.contains(cast), "{cast} in {text}");
        }
    }
}
