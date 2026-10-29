//! The Cranelift backend: validated Ready LIR units lowered to Cranelift IR and emitted as one
//! relocatable ELF object, for fast debug builds.
//!
//! It compiles the same LIR as `hirsel-llvm`, against the same backend-neutral interface
//! (`hirsel_lir::emission`): the same `EmitInputs`, link names, failure codes, site codes,
//! runtime descriptors, static blocks and Hirsel ABI v3, so its objects link with the same
//! runtime archive and launcher. It keeps none of the LLVM backend's optimizations (no
//! compiled drop glue, inline allocation fast paths, deferred counts or private root
//! returns): every count edge, allocation and release goes through its runtime entry. A
//! Ready operation it does not lower is `CodegenError::Unsupported` naming it, never a
//! placeholder.

#[cfg(test)]
hirsel_guard::install!();

mod ahead;
mod bounded;
mod compare;
mod construction;
mod data;
mod dispatcher;
mod drop;
mod dwarf;
pub use dwarf::{Resolved, debug_object, debug_object_sources};
mod flat;
mod foreign;
mod function;
mod inline;
mod instructions;
mod module;
mod object;
mod ownership_trace;
mod places;
mod profile;
mod program;
mod receipt;
mod resume;
mod root_facts;
mod scalar;
mod sealed;
mod soa;
mod structural;
#[cfg(feature = "native-tsan")]
mod tsan;

use std::fmt;

use hirsel_lir::emission::{EmitInputs, GlueInputs};
use hirsel_lir::stage::{ObjectArtifact, Ready, ValidatedUnit};
use hirsel_syntax::DefPath;

/// The pinned Cranelift release (`=0.136.3` in this crate's manifest).
pub const CRANELIFT_VERSION: &str = "0.136.3";

/// Whether every emitted object includes ThreadSanitizer instrumentation.
pub const NATIVE_TSAN: bool = cfg!(feature = "native-tsan");

/// The backend an artifact names in its reproducibility record (`Reproducibility::toolchain`).
pub const TOOLCHAIN: &str = if cfg!(feature = "native-tsan") {
    "cranelift 0.136.3 tsan"
} else {
    "cranelift 0.136.3"
};

/// Why emission produced no object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodegenError {
    /// Cranelift has no ISA for the triple, or its flags are refused.
    Target { triple: String, message: String },
    /// A unit's receipt does not bind the inputs emission was given.
    Receipt {
        definition: DefPath,
        problem: String,
    },
    /// A validated unit is inconsistent with its tables: an emitter or validator fault.
    Invalid { definition: DefPath, what: String },
    /// A Ready operation this emitter does not lower.
    Unsupported { definition: DefPath, what: String },
    /// Two units have one link name.
    DuplicateSymbol { name: String },
    /// Cranelift refused a function or a declaration.
    Cranelift { name: String, message: String },
    /// The generated object is not a relocatable ELF object for the target.
    Object { message: String },
    /// The object's symbol interface disagrees with the manifest emission declared.
    Manifest { problem: String },
    /// Code generation panicked.
    Panic { message: String },
}

impl CodegenError {
    /// The definition the error is about, when it is about one.
    pub fn definition(&self) -> Option<&DefPath> {
        match self {
            CodegenError::Receipt { definition, .. }
            | CodegenError::Invalid { definition, .. }
            | CodegenError::Unsupported { definition, .. } => Some(definition),
            _ => None,
        }
    }
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::Target { triple, message } => {
                write!(f, "no Cranelift target for {triple}: {message}")
            }
            CodegenError::Receipt {
                definition,
                problem,
            } => write!(f, "{definition}: {problem}"),
            CodegenError::Invalid { definition, what } => {
                write!(f, "{definition}: invalid Ready unit: {what}")
            }
            CodegenError::Unsupported { definition, what } => {
                write!(
                    f,
                    "{definition}: the Cranelift backend does not support {what}"
                )
            }
            CodegenError::DuplicateSymbol { name } => write!(f, "two units are named `{name}`"),
            CodegenError::Cranelift { name, message } => {
                write!(f, "Cranelift refused `{name}`: {message}")
            }
            CodegenError::Object { message } => write!(f, "malformed object: {message}"),
            CodegenError::Manifest { problem } => write!(f, "object manifest mismatch: {problem}"),
            CodegenError::Panic { message } => {
                write!(f, "the Cranelift backend panicked: {message}")
            }
        }
    }
}

impl std::error::Error for CodegenError {}

/// Emits the program's glue object (`hirsel_lir::glue`): the descriptor registry, the
/// index and failure-code cells, codec records, statics, literals and C callback thunks
/// the program's unit objects import.
pub fn emit_glue(inputs: &GlueInputs<'_>) -> Result<ObjectArtifact, CodegenError> {
    caught(|| program::emit_glue(inputs))
}

/// Emits validated Ready units as one relocatable object for `inputs.target`.
pub fn emit_object(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| module::emit(units, inputs, false, Default::default(), None))
        .map(|(artifact, _, _)| artifact)
}

/// Emits `units` as `emit_object` does, recording what `profile` asks for beside the code:
/// each function's debug intervals (`ObjectArtifact::debug`), for the link's
/// `debug_object`. Cranelift embeds no measurement instrumentation.
pub fn emit_object_profile(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    profile: hirsel_lir::debug::Profile<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| module::emit(units, inputs, false, profile, None)).map(|(artifact, _, _)| artifact)
}

/// The Cranelift IR of every function `emit_object` defines, before compilation, as text:
/// what a parity failure is debugged from.
pub fn emit_clif(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    caught(|| module::emit(units, inputs, true, Default::default(), None))
        .map(|(_, clif, _)| clif.unwrap_or_default())
}

/// Emit diagnostic shadow hooks for the supported leaf ownership subset.
/// Unsupported transfers are refused before any object is published.
pub fn emit_object_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| {
        module::emit(
            units,
            inputs,
            false,
            Default::default(),
            Some(hirsel_lir::ownership_trace::native::Request::Leaf),
        )
    })
    .map(|(artifact, _, _)| artifact)
}

/// Cranelift IR with explicit diagnostic ownership calls.
pub fn emit_clif_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    caught(|| {
        module::emit(
            units,
            inputs,
            true,
            Default::default(),
            Some(hirsel_lir::ownership_trace::native::Request::Leaf),
        )
    })
    .map(|(_, clif, _)| clif.unwrap_or_default())
}

/// Emit exactly the plans preserved by earlier compiler passes.
pub fn emit_object_relocated_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &std::collections::BTreeMap<
        hirsel_syntax::ContentHash,
        hirsel_lir::ownership_trace::Plan,
    >,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| {
        module::emit(
            units,
            inputs,
            false,
            Default::default(),
            Some(hirsel_lir::ownership_trace::native::Request::Relocated(
                plans,
            )),
        )
    })
    .map(|(artifact, _, _)| artifact)
}

/// Emit the explicit resource selection of a source diagnostic request.
pub fn emit_object_resources_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &std::collections::BTreeMap<
        hirsel_syntax::ContentHash,
        hirsel_lir::ownership_trace::Plan,
    >,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| {
        module::emit(
            units,
            inputs,
            false,
            Default::default(),
            Some(hirsel_lir::ownership_trace::native::Request::Resources(
                plans,
            )),
        )
    })
    .map(|(artifact, _, _)| artifact)
}

pub fn emit_clif_relocated_ownership(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &std::collections::BTreeMap<
        hirsel_syntax::ContentHash,
        hirsel_lir::ownership_trace::Plan,
    >,
) -> Result<String, CodegenError> {
    caught(|| {
        module::emit(
            units,
            inputs,
            true,
            Default::default(),
            Some(hirsel_lir::ownership_trace::native::Request::Relocated(
                plans,
            )),
        )
    })
    .map(|(_, clif, _)| clif.unwrap_or_default())
}

/// A panic in Cranelift or in this crate is a refused compilation, never a crash of the
/// caller nor an object.
fn caught<T>(emit: impl FnOnce() -> Result<T, CodegenError>) -> Result<T, CodegenError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(emit)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic without a message".into());
        Err(CodegenError::Panic { message })
    })
}

/// Compiles the launcher's `main`: it ignores SIGPIPE, then calls `launcher` (a runtime
/// launch entry of `hirsel_rt_v2`) with the entry adapter `hirsel.entry` and `arguments`,
/// where the adapter calls `entry` with `(ctx, dest, out)` and one pointer per root,
/// root `i` being the word at `roots + 8 * root_words[i]`.
pub use module::{
    DescriptorField, ExitReport, ReportNode, named_callee_launch_object, named_launch_object,
};

/// A package-test `main` selecting one named `(argc, argv) -> exit_code` entry.
/// Invalid selections return 3, matching the LLVM dispatcher.
pub fn test_dispatcher_object(
    target: &hirsel_lir::layout_types::TargetSpec,
    entries: &[&str],
) -> Result<Vec<u8>, CodegenError> {
    caught(|| dispatcher::emit(target, entries))
}

/// Emits real alternate definitions and reads their final x86 SysV frame metadata.
pub fn emit_ahead_object(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<(ObjectArtifact, std::collections::BTreeMap<String, u64>), CodegenError> {
    if inputs.ahead.is_some_and(|plan| {
        !hirsel_lir::ahead::validate_cpu_workers(
            units,
            inputs.layouts,
            inputs.target,
            plan,
            inputs.fuel,
        )
    }) {
        return Err(CodegenError::Object {
            message: "invalid CPU ahead certificate".into(),
        });
    }
    let inputs = EmitInputs {
        alternate: true,
        available_externally: &Default::default(),
        ..inputs.clone()
    };
    caught(|| module::emit(units, &inputs, false, Default::default(), None))
        .map(|(object, _, frames)| (object, frames))
}

/// Emits the continuation alternates and dispatchers of every resumable plan.
pub fn emit_resume_object(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    plans: &hirsel_lir::continuation::ContinuationPlans,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| resume::emit(units, inputs, plans))
}

/// Emits backend-independent immutable scalar capture and frame records.
pub fn emit_ahead_metadata(
    inputs: &EmitInputs<'_>,
    plan: &hirsel_lir::ahead::AheadPlan,
    bounds: &std::collections::BTreeMap<hirsel_syntax::ContentHash, u64>,
    registry: &hirsel_lir::descriptor_registry::DescriptorRegistry,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| ahead::metadata(inputs, plan, bounds, registry))
}
