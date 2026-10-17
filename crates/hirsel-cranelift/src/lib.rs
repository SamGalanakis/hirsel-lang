//! The Cranelift backend: validated Ready LIR units lowered to Cranelift IR and emitted as one
//! relocatable ELF object, for fast debug builds.
//!
//! It compiles the same LIR as `hirsel-llvm`, against the same backend-neutral interface
//! (`hirsel_lir::emission`): the same `EmitInputs`, link names, failure codes, site codes,
//! runtime descriptors, static blocks and Hirsel ABI v1, so its objects link with the same
//! runtime archive and launcher. It keeps none of the LLVM backend's optimizations (no
//! compiled drop glue, inline allocation fast paths, deferred counts or private root
//! returns): every count edge, allocation and release goes through its runtime entry. A
//! Ready operation it does not lower is `CodegenError::Unsupported` naming it, never a
//! placeholder.

mod compare;
mod construction;
mod data;
mod drop;
mod flat;
mod function;
mod instructions;
mod module;
mod places;
mod scalar;

use std::fmt;

use hirsel_lir::emission::EmitInputs;
use hirsel_lir::stage::{ObjectArtifact, Ready, ValidatedUnit};
use hirsel_syntax::DefPath;

/// The pinned Cranelift release (`=0.136.3` in this crate's manifest).
pub const CRANELIFT_VERSION: &str = "0.136.3";

/// The backend an artifact names in its reproducibility record (`Reproducibility::toolchain`).
pub const TOOLCHAIN: &str = "cranelift 0.136.3";

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
    /// A failure edge starts a rule with no runtime failure code.
    UnknownRule { definition: DefPath, rule: String },
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
            | CodegenError::Unsupported { definition, .. }
            | CodegenError::UnknownRule { definition, .. } => Some(definition),
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
            CodegenError::UnknownRule { definition, rule } => {
                write!(f, "{definition}: no runtime failure code for {rule}")
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

/// Emits validated Ready units as one relocatable object for `inputs.target`.
pub fn emit_object(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<ObjectArtifact, CodegenError> {
    caught(|| module::emit(units, inputs, false)).map(|(artifact, _)| artifact)
}

/// The Cranelift IR of every function `emit_object` defines, before compilation, as text:
/// what a parity failure is debugged from.
pub fn emit_clif(
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
) -> Result<String, CodegenError> {
    caught(|| module::emit(units, inputs, true)).map(|(_, clif)| clif.unwrap_or_default())
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
pub use module::{LaunchArgument, launch_object};
