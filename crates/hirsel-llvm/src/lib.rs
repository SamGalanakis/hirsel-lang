//! The LLVM backend: owned LLVM handles behind a small RAII layer, and emission of validated
//! Ready LIR units that returns an owned `hirsel_lir::ObjectArtifact` after every LLVM and
//! LIR temporary is dropped.
//!
//! The binding is llvm-sys `=231.0.1` against the shared libLLVM 23.1.4 pinned in
//! `tools/native-toolchain/llvm-23.lock`. No raw LLVM handle is
//! public, and there is no fallback to another LLVM.

#[cfg(test)]
hirsel_guard::install!();

mod c_entry;
mod compare;
mod context;
mod debug;
mod emit;
mod glue;
mod runtime;
mod scalar;

use std::fmt;

use hirsel_syntax::DefPath;

pub use c_entry::{c_call, c_definition};
pub use emit::{check_target, compile_ir, emit_ir, emit_object, emit_optimized_ir};
pub use emit::{
    emit_glue, emit_ir_ownership, emit_ir_relocated_ownership, emit_object_ownership,
    emit_object_profile, emit_object_relocated_ownership, emit_resume_object,
};
pub use hirsel_lir::profile::{ProfileSource, ProfileSources};

/// An LLVM release, `major.minor.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LlvmVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for LlvmVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The pinned LLVM release (`llvm_version` in `tools/native-toolchain/llvm-23.lock`).
pub const PINNED_LLVM: LlvmVersion = LlvmVersion {
    major: 23,
    minor: 1,
    patch: 4,
};

/// The version of the libLLVM this process loaded.
pub fn loaded_llvm_version() -> LlvmVersion {
    context::loaded_version()
}

/// Where in the pipeline the LLVM verifier ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyStage {
    BeforeOptimization,
    AfterOptimization,
}

/// Why emission produced no object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodegenError {
    /// The loader resolved a libLLVM other than the pin.
    LoadedLlvm { loaded: LlvmVersion },
    /// LLVM has no usable target machine for the triple.
    TargetMachine { triple: String, message: String },
    /// LLVM's DataLayout disagrees with a fact `TargetSpec` pins.
    DataLayout {
        triple: String,
        what: String,
        expected: String,
        actual: String,
    },
    /// The textual IR names a different target triple.
    Triple { expected: String, actual: String },
    /// The textual IR does not parse.
    Parse { message: String },
    /// A unit's receipt does not bind the inputs emission was given.
    Receipt {
        definition: DefPath,
        problem: String,
    },
    /// A validated unit is inconsistent with its tables: an emitter or validator fault.
    Invalid { definition: DefPath, what: String },
    /// A Ready operation this emitter does not implement yet.
    Unsupported { definition: DefPath, what: String },
    /// The program's glue object cannot be built from its inputs.
    Glue { what: String },
    /// Two units have one link name.
    DuplicateSymbol { name: String },
    /// The LLVM verifier rejected the module.
    Verify { stage: VerifyStage, message: String },
    /// The optimization pipeline failed.
    Passes { message: String },
    /// Code generation failed.
    Emit { message: String },
    /// The generated object is not a relocatable ELF object for the target.
    Object { message: String },
    /// The object's symbol interface disagrees with the manifest emission declared.
    Manifest { problem: String },
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::LoadedLlvm { loaded } => {
                write!(
                    f,
                    "the loaded libLLVM is {loaded}, the pin is {PINNED_LLVM}"
                )
            }
            CodegenError::TargetMachine { triple, message } => {
                write!(f, "no LLVM target machine for {triple}: {message}")
            }
            CodegenError::DataLayout {
                triple,
                what,
                expected,
                actual,
            } => write!(
                f,
                "LLVM DataLayout for {triple} disagrees on the {what}: `{actual}`, expected `{expected}`"
            ),
            CodegenError::Triple { expected, actual } => {
                write!(f, "the module targets `{actual}`, not {expected}")
            }
            CodegenError::Parse { message } => write!(f, "LLVM IR does not parse: {message}"),
            CodegenError::Receipt {
                definition,
                problem,
            } => {
                write!(f, "{}: {problem}", definition)
            }
            CodegenError::Invalid { definition, what } => {
                write!(f, "{}: invalid Ready unit: {what}", definition)
            }
            CodegenError::Unsupported { definition, what } => {
                write!(
                    f,
                    "{}: native emission does not support {what} yet",
                    definition
                )
            }
            CodegenError::Glue { what } => write!(f, "program glue: {what}"),
            CodegenError::DuplicateSymbol { name } => write!(f, "two units are named `{name}`"),
            CodegenError::Verify { stage, message } => {
                let stage = match stage {
                    VerifyStage::BeforeOptimization => "before",
                    VerifyStage::AfterOptimization => "after",
                };
                write!(f, "LLVM verifier failed {stage} optimization: {message}")
            }
            CodegenError::Passes { message } => write!(f, "LLVM optimization failed: {message}"),
            CodegenError::Emit { message } => write!(f, "LLVM code generation failed: {message}"),
            CodegenError::Object { message } => write!(f, "malformed object: {message}"),
            CodegenError::Manifest { problem } => write!(f, "object manifest mismatch: {problem}"),
        }
    }
}

impl std::error::Error for CodegenError {}

/// Emits real alternate definitions and reads final caller-SP frame extents.
pub fn emit_ahead_object(
    units: &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
    inputs: &hirsel_lir::emission::EmitInputs<'_>,
) -> Result<
    (
        hirsel_lir::stage::ObjectArtifact,
        std::collections::BTreeMap<String, u64>,
    ),
    CodegenError,
> {
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
    let inputs = hirsel_lir::emission::EmitInputs {
        alternate: true,
        available_externally: &Default::default(),
        ..inputs.clone()
    };
    let object = emit_object(units, &inputs)?;
    let frames = hirsel_lir::object::llvm_stack_sizes(&object.bytes)
        .map_err(|message| CodegenError::Object { message })?;
    Ok((object, frames))
}
