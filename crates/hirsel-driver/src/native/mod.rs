//! Native stages: the frozen snapshot is
//! adapted into the plain program the native passes read, lowered stage by
//! stage with every stage validator invoked, and the Ready program is run by
//! the checked heap executor or emitted, linked and launched.
//!
//! A native failure is a tool outcome (`NativeError`), never a source verdict.
mod adapt;
mod admission;
mod coverage;
mod differential;
mod emit;
mod executable;
mod exit_report;
mod heap;
mod import_cache;
mod kit;
mod link;
mod observe;
mod pairwise;
mod proofs;
mod runner;
mod runtime_cache;
pub use runtime_cache::RuntimeInputs;
mod stages;
mod std_seed;
mod std_surface;
pub use std_seed::{SeedReport, seed_objects};
pub use std_surface::StdSurface;
mod testing;
mod timeout;
mod tool;
mod unit_bodies;
mod units;
pub use testing::{bench_path, test_path};
pub use units::UnitObjects;

pub use adapt::{input_identity, manifest, program};
pub use coverage::backend_features;
pub use differential::{
    Configuration, Engine, Fixture, MATRIX_VERSION, Outcome, Runtime, differential,
    differential_in, full_matrix, real_io_replay, real_io_replay_in, real_io_replay_with_config,
    runner_oracle_in, runtime_builds, smoke_matrix, topology_matrix, trace_roots_matrix,
};
pub use emit::{
    Backend, KIND_BUG, check_runtime_calls, emit, emit_clif, emit_glue, emit_ir, failure_codes,
    failure_rules,
};
pub use executable::{
    BuildProfile, build_path_native, build_path_native_backend,
    build_path_native_backend_with_validation,
};
pub use heap::{HeapRun, HeapWorld, heap_campaign, heap_campaign_in, run_heap, run_heap_in};
pub use hirsel_lir::emission::OptLevel;
pub use hirsel_lir::layout_types::TargetSpec;
pub use hirsel_lir::profile::{ProfileSource, ProfileSources};
pub use hirsel_llvm::c_call;
pub use hirsel_proof_export::sidecar::Checker;
pub use hirsel_run_schema::Inputs as LaunchInputs;
pub use link::{
    Launched, Library, Observed, Partition, Runnable, RuntimeBuild, SMALL_UNIT_INSTRUCTIONS, Tape,
    Toolchain, emit_backend_objects, emit_objects, launch, launch_runnable, launch_with, link,
    link_host, link_library, link_objects, link_profile, link_runnable, observe, observe_run,
    partition_backend_objects, partition_backend_objects_with_jobs, partition_objects, run_host,
};
pub use observe::{Failure, OBSERVATION_SCHEMA, Observation, Sites, site_codes};
pub use pairwise::pairwise_matrix;
pub use proofs::{Evidence, Proofs, parse as parse_sidecar};
pub use runner::{Counts, Execution, Image, Mode, Runner, counts, runner};
pub use stages::{NativeError, Options, ReadyProgram, StageName, Validation, build, build_program};
pub use tool::{
    Output, run_path_native, run_path_native_backend, run_path_native_backend_with_validation,
    run_path_native_with_config,
};

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
            let (file, base) = input.sources.anchor(owner.syntax.anchor)?;
            let file = input.sources.file(file)?;
            Some((
                definition.clone(),
                ProfileSource {
                    path: file.path.clone(),
                    file: file.clone(),
                    base,
                    anchors: input
                        .definitions
                        .get(definition)
                        .map_or_else(
                            || vec![owner.syntax.anchor],
                            |d| hirsel_tir::source_anchors(d),
                        )
                        .into_iter()
                        .filter_map(|anchor| {
                            let (file, base) = input.sources.anchor(anchor)?;
                            Some((input.sources.file(file)?.clone(), base))
                        })
                        .collect(),
                },
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/native-validation/smoke.rs"]
mod smoke_validation_tests;
#[cfg(test)]
#[path = "../../tests/native-validation/roots.rs"]
mod validation_tests;
