//! Native stages: the frozen snapshot is
//! adapted into the plain program the native passes read, lowered stage by
//! stage with every stage validator invoked, and the Ready program is run by
//! the checked heap executor or emitted, linked and launched.
//!
//! A native failure is a tool outcome (`NativeError`), never a source verdict.
mod adapt;
mod differential;
mod emit;
mod heap;
mod link;
mod observe;
mod runtime_cache;
mod stages;
mod tool;

pub use adapt::{input_identity, manifest, program};
pub use differential::{
    Configuration, Engine, MATRIX_VERSION, Outcome, Runtime, differential, full_matrix,
    real_io_replay, real_io_replay_with_config, runtime_builds, smoke_matrix, topology_matrix,
};
pub use emit::{KIND_BUG, check_runtime_calls, emit, emit_ir, failure_codes, failure_rules};
pub use heap::{HeapRun, heap_campaign, run_heap};
pub use hirsel_lir::layout_types::TargetSpec;
pub use hirsel_llvm::{OptLevel, ProfileSource, ProfileSources, c_call};
pub use hirsel_run_config::Inputs as LaunchInputs;
pub use link::{
    Launched, MEMCHECK_EXIT, Partition, RuntimeBuild, SMALL_UNIT_INSTRUCTIONS, Tape, Toolchain,
    emit_objects, launch, launch_with, link, link_objects, link_profile, observe,
    partition_objects,
};
pub use observe::{Failure, OBSERVATION_SCHEMA, Observation, Sites, site_codes};
pub use stages::{NativeError, Options, ReadyProgram, StageName, build, build_program};
pub use tool::run_path_native;

/// The pinned LLVM release emission binds, for reports and artifacts.
pub fn llvm_version() -> String {
    hirsel_llvm::PINNED_LLVM.to_string()
}

/// Retain immutable source ownership for line/site profiling without querying a backend.
pub fn profile_sources(input: &hirsel_engine::NativeInput) -> ProfileSources {
    input
        .source_owners
        .iter()
        .filter_map(|(definition, owner)| {
            let file = input.sources.file(owner.syntax.file)?;
            Some((definition.clone(), file.clone()))
        })
        .collect()
}
