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
mod file_digest;
mod foreign;
mod heap;
mod import_cache;
mod kit;
mod link;
pub mod object_queries;
mod observe;
mod pairwise;
mod plan;
mod proofs;
pub mod queries;
mod runner;
mod runtime_cache;
pub use runtime_cache::RuntimeInputs;
mod stages;
mod std_surface;
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
    differential_in, full_matrix, migrate_matrix, real_io_replay, real_io_replay_in,
    real_io_replay_with_config, runner_oracle_in, runtime_builds, smoke_matrix, topology_matrix,
    trace_roots_matrix,
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
    Toolchain, emit_backend_objects, emit_objects, launch, launch_fixture, launch_runnable,
    launch_with, link, link_fixture_runnable, link_host, link_library, link_objects, link_profile,
    link_runnable, link_standalone_objects, observe, observe_run, partition_backend_objects,
    partition_backend_objects_with_jobs, partition_objects, run_host,
};
pub use observe::{Failure, OBSERVATION_SCHEMA, Observation, Sites, site_codes};
pub use pairwise::pairwise_matrix;
pub use proofs::{Evidence, Proofs, parse as parse_sidecar};
pub use runner::{Counts, Execution, Image, Mode, Runner, counts, runner};
pub use stages::{NativeError, Options, ReadyProgram, StageName, Validation, build, build_program};
pub use tool::{
    Output, run_path_native, run_path_native_backend, run_path_native_backend_with_trace_events,
    run_path_native_backend_with_validation, run_path_native_with_config,
};

/// The pinned LLVM release emission binds, for reports and artifacts.
pub fn llvm_version() -> String {
    hirsel_llvm::PINNED_LLVM.to_string()
}

/// For a generated (`= comptime`) body, the item-relative offset of its
/// `comptime` expression, where its sites are reported: they are offsets into the
/// generated text, not into the item's file.
fn generator_site(syntax: &hirsel_syntax::ItemSyntax) -> Option<u32> {
    let hirsel_syntax::ItemKind::Function {
        body: Some(body), ..
    } = syntax.item.kind
    else {
        return None;
    };
    let body = &syntax.tree.expressions[body.0 as usize];
    matches!(body.kind, hirsel_syntax::ExprKind::Comptime(_)).then(|| {
        let span = body.origin.span;
        if span.anchor == syntax.anchor {
            span.start
        } else {
            0
        }
    })
}

/// Retain immutable source ownership for line/site profiling without querying a backend.
pub fn profile_sources(input: &hirsel_engine::NativeInput) -> ProfileSources {
    profile_sources_for_owners(
        &input.sources,
        input.source_owners.iter().map(|(definition, owner)| {
            let anchors = input.definitions.get(definition).map_or_else(
                || vec![owner.syntax.anchor],
                |body| hirsel_tir::source_anchors(body),
            );
            (definition, owner, anchors)
        }),
    )
}

/// The same line/source projection over compact native declaration products.
/// Reading canonical source anchors never restores a Definition or native body.
pub fn declaration_profile_sources(
    declarations: &queries::declarations::NativeDeclarations,
) -> ProfileSources {
    declaration_profile_sources_in(declarations, &declarations.roots.sources)
}

pub(crate) fn declaration_profile_sources_in(
    declarations: &queries::declarations::NativeDeclarations,
    sources: &hirsel_span::SourceMap,
) -> ProfileSources {
    profile_sources_for_owners(
        sources,
        declarations.sources.iter().map(|(definition, product)| {
            let anchors = declarations
                .source_anchors
                .get(definition)
                .cloned()
                .unwrap_or_else(|| vec![product.source.syntax.anchor]);
            (definition, &product.source, anchors)
        }),
    )
}

fn profile_sources_for_owners<'a>(
    sources: &hirsel_span::SourceMap,
    owners: impl Iterator<
        Item = (
            &'a hirsel_syntax::DefPath,
            &'a hirsel_engine::NativeSource,
            Vec<hirsel_span::Anchor>,
        ),
    >,
) -> ProfileSources {
    owners
        .filter_map(|(definition, owner, anchors)| {
            let (file, base) = sources.anchor(owner.syntax.anchor)?;
            let file = sources.file(file)?;
            Some((
                definition.clone(),
                ProfileSource {
                    path: file.path.clone(),
                    file: file.clone(),
                    base,
                    generated: generator_site(&owner.syntax),
                    anchors: anchors
                        .into_iter()
                        .filter_map(|anchor| {
                            let (file, base) = sources.anchor(anchor)?;
                            Some((sources.file(file)?.clone(), base))
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

/// Checked registry schemas accompany native runner metadata, so request hashes
/// use the same SER-2 envelopes as interpreted and remote calls.
fn callee_metadata(
    engine: &mut hirsel_engine::QueryEngine,
    entry: &hirsel_engine::DefPath,
) -> Result<hirsel_rt::program::CalleeMetadata, NativeError> {
    let failed = |error: String| NativeError::new(StageName::Launch, error);
    let facts = hirsel_engine::callee_facts(engine, entry)
        .map_err(|e| failed(format!("callee facts: {e:?}")))?;
    let runtime = engine
        .runtime_types()
        .map_err(|e| failed(format!("runner request types: {e:?}")))?;
    let schemas = hirsel_interp::library::runners::core_schema(&runtime, "GrantSet")
        .and_then(|grants| {
            hirsel_interp::library::runners::allowlist_schema(&runtime)
                .map(|definitions| (grants, definitions))
        })
        .map_err(|e| failed(format!("runner request schemas: {e:?}")))?;
    let descriptor = |schema: &hirsel_types::serialization::SerializableSchema| {
        hirsel_types::wire::Codec::new(schema)
            .map(|codec| codec.descriptor().to_vec())
            .map_err(|e| failed(format!("runner request codec: {e:?}")))
    };
    let request_schemas = hirsel_rt::program::RunnerRequestSchemas::new(
        descriptor(&schemas.0)?,
        descriptor(&schemas.1)?,
    )
    .map_err(failed)?;
    Ok(hirsel_rt::program::CalleeMetadata {
        facts,
        request_schemas: Some(request_schemas),
    })
}

#[cfg(test)]
mod declaration_profile_tests {
    #[test]
    fn compact_profile_sources_match_original_generated_and_closure_anchors() {
        use super::queries::{
            declarations::NativeDeclarations, source_inputs::NativeInstanceInputs,
        };
        use hirsel_engine::{
            DefinitionQueries, NativeDiscoveryCache, QueryEngine, default_context,
            default_environment,
        };
        hirsel_engine::with_compiler_stack(|| {
            let store = std::sync::Arc::new(hirsel_engine::store::MemStore::default());
            let source = "type P {n:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String] {Ok(codegen.body(\"fn(v:standalone@0/P)->u64 uses {} {v.n}\"))}\nfn chosen[T](v:T)->u64 uses {} = comptime gen(shape(T))\nfn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\npub fn main()->u64 uses {} {let n=chosen(P(n:3u64));invoke(fn()->u64 uses {}{n})}";
            let mut engine = QueryEngine::new(default_environment(), crate::compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("debug-projection.hir", source.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
            let inputs = NativeInstanceInputs::complete(&mut engine, declarations, store).unwrap();
            assert!(inputs.declarations.program.definitions.is_empty());
            let actual = super::declaration_profile_sources(&inputs.declarations);
            let input = engine.export_native_input().unwrap();
            let expected = super::profile_sources(&input);
            assert_eq!(
                actual.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>()
            );
            assert!(actual.values().any(|source| source.generated.is_some()));
            for (path, actual) in actual {
                let expected = &expected[&path];
                assert_eq!(actual.path, expected.path);
                assert_eq!(actual.logical_path(), expected.logical_path());
                assert_eq!(actual.file.text(), expected.file.text());
                assert_eq!(
                    (actual.base, actual.generated),
                    (expected.base, expected.generated)
                );
                let anchors = |source: &super::ProfileSource| {
                    source
                        .anchors
                        .iter()
                        .map(|(file, base)| {
                            (
                                file.logical_path().to_owned(),
                                file.text().to_string(),
                                *base,
                            )
                        })
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    anchors(&actual),
                    anchors(expected),
                    "source selectors for {path}"
                );
            }
        });
    }
}
