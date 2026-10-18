//! Native stages: the frozen snapshot is
//! adapted into the plain program the native passes read, lowered stage by
//! stage with every stage validator invoked, and the Ready program is run by
//! the checked heap executor or emitted, linked and launched.
//!
//! A native failure is a tool outcome (`NativeError`), never a source verdict.
mod adapt;
mod coverage;
mod differential;
mod emit;
mod executable;
mod exit_report;
mod heap;
mod link;
mod observe;
mod pairwise;
mod proofs;
mod runtime_cache;
mod stages;
mod testing;
mod tool;
pub use testing::{bench_path, test_path};

pub use adapt::{input_identity, manifest, program};
pub use coverage::backend_features;
pub use differential::{
    Configuration, Engine, Fixture, MATRIX_VERSION, Outcome, Runtime, differential,
    differential_in, full_matrix, real_io_replay, real_io_replay_in, real_io_replay_with_config,
    runtime_builds, smoke_matrix, topology_matrix,
};
pub use emit::{
    Backend, KIND_BUG, check_runtime_calls, emit, emit_clif, emit_ir, failure_codes, failure_rules,
};
pub use executable::{BuildProfile, build_path_native, build_path_native_backend};
pub use heap::{HeapRun, HeapWorld, heap_campaign, heap_campaign_in, run_heap, run_heap_in};
pub use hirsel_lir::emission::OptLevel;
pub use hirsel_lir::layout_types::TargetSpec;
pub use hirsel_lir::profile::{ProfileSource, ProfileSources};
pub use hirsel_llvm::c_call;
pub use hirsel_proof_export::sidecar::Checker;
pub use hirsel_run_config::Inputs as LaunchInputs;
pub use link::{
    Launched, Partition, RuntimeBuild, SMALL_UNIT_INSTRUCTIONS, Tape, Toolchain,
    emit_backend_objects, emit_objects, launch, launch_with, link, link_objects, link_profile,
    observe, partition_backend_objects, partition_objects,
};
pub use observe::{Failure, OBSERVATION_SCHEMA, Observation, Sites, site_codes};
pub use pairwise::pairwise_matrix;
pub use proofs::{Evidence, Proofs, parse as parse_sidecar};
pub use stages::{NativeError, Options, ReadyProgram, StageName, build, build_program};
pub use tool::{Output, run_path_native, run_path_native_backend, run_path_native_with_config};

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
