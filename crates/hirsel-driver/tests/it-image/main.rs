//! The std image against a fresh check of std (docs/building.md, "The std
//! precheck"): the one test that needs the toolchain's precheck
//! (`HIRSEL_PRODUCTS`). It checks std from source once, so it links as its own
//! binary with its own measured request.

hirsel_guard::install!();

#[path = "../support/image_fixture.rs"]
mod image_fixture;

use hirsel_driver::*;
use hirsel_engine::*;
use image_fixture::*;

const STD_PROGRAM: &str = "pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  out.print(output, string.trim(\" hello \")) ? Output
  out.print(output, u64.to_string(42u64)) ? Output
  Ok(())
}
";

#[path = "../support/canonical_debug.rs"]
mod canonical_debug;
use canonical_debug::{canonical_ids, placed};

use hirsel_driver::requests::*;
use hirsel_engine::runtime::ProcessBudget;
use std::{path::Path, sync::Arc};

fn requests(path: &Path, products: Option<&Path>) -> CompilerRequests {
    let store: Arc<dyn store::ResultStore> = match products {
        Some(products) => Arc::new(
            store::DirStore::new(path.parent().unwrap().join("immutable-reader"))
                .with_read_only_root(products.to_owned()),
        ),
        None => Arc::new(store::NoneStore),
    };
    let config = CompileConfig {
        store,
        jobs: hirsel_intern::jobs::Jobs::ONE,
        ..CompileConfig::for_memory_cap(None)
    };
    CompilerRequests::for_host(COMPILER.with_config(config.clone()), config)
}

fn precompute(requests: &mut CompilerRequests, path: &Path) -> Arc<PrecomputedSources> {
    let output = requests
        .execute(path, CompileOperation::Precompute)
        .unwrap();
    let CompileResult::Precomputed { sources, .. } = &output.result else {
        panic!("expected complete source products")
    };
    sources.clone()
}

fn hashes(
    requests: &mut CompilerRequests,
    path: &Path,
) -> std::collections::BTreeMap<DefPath, hirsel_syntax::ContentHash> {
    let mut all = std::collections::BTreeMap::new();
    for package in [Some(package("std")), None] {
        let output = requests
            .execute(path, CompileOperation::Hashes { package })
            .unwrap();
        let CompileResult::Hashes(hashes) = &output.result else {
            panic!("expected hashes")
        };
        all.extend(hashes.iter().map(|(path, hash)| (path.clone(), *hash)));
    }
    all
}

fn checked(requests: &mut CompilerRequests, path: &Path) -> (CheckCompletion, Vec<String>) {
    let output = requests
        .execute(
            path,
            CompileOperation::Check {
                lowering: LoweringPolicy::Reachable,
            },
        )
        .unwrap();
    let CompileResult::Check(output) = &output.result else {
        panic!("expected Check")
    };
    (
        output.completion,
        output
            .diagnostics
            .iter()
            .map(|d| d.render_human(&output.sources))
            .collect(),
    )
}

/// Every prechecked std definition hashes the same (LANG-6), lowers to the same TIR
/// and has the same revision node as in a fresh check, in a program that places
/// std's sources at other FileIds than the program the precheck was checked in.
#[test]
fn ordinary_std_complete_products_equal_fresh_ones() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let products = std_products();
        let client = f.program("aaa", STD_PROGRAM, &std_manifest(), "std");
        let mut warm_requests = requests(&client, Some(&products));
        let mut cold_requests = requests(&client, None);
        let actual_hashes = hashes(&mut warm_requests, &client);
        let expected_hashes = hashes(&mut cold_requests, &client);
        assert!(expected_hashes.len() > 1000, "{}", expected_hashes.len());
        let differing: Vec<_> = expected_hashes
            .iter()
            .filter(|(path, hash)| actual_hashes.get(*path) != Some(*hash))
            .map(|(path, _)| path.to_string())
            .collect();
        assert!(differing.is_empty(), "{differing:?}");
        assert_eq!(actual_hashes.len(), expected_hashes.len());
        assert_eq!(
            checked(&mut warm_requests, &client),
            checked(&mut cold_requests, &client)
        );
        let prechecked = precompute(&mut warm_requests, &client);
        let reference = precompute(&mut cold_requests, &client);
        let definitions = prechecked
            .definitions()
            .filter(|p| p.0.package == package("std"))
            .cloned()
            .collect::<Vec<_>>();
        let graph_bytes = prechecked.semantic_graph_decode_bytes().unwrap()
            + reference.semantic_graph_decode_bytes().unwrap();
        let graph_budget = ProcessBudget::new(graph_bytes);
        let actual_graph = prechecked
            .semantic_graph(
                graph_budget
                    .reserve(prechecked.semantic_graph_decode_bytes().unwrap())
                    .unwrap(),
            )
            .unwrap();
        let expected_graph = reference
            .semantic_graph(
                graph_budget
                    .reserve(reference.semantic_graph_decode_bytes().unwrap())
                    .unwrap(),
            )
            .unwrap();
        let entry = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("aaa"),
            modules: vec![],
            name: "main".into(),
        });
        assert_eq!(
            actual_graph.revision(&entry).unwrap(),
            expected_graph.revision(&entry).unwrap()
        );
        let render = |products: &PrecomputedSources, value: &hirsel_tir::Definition| {
            placed(
                products.sources(),
                canonical_ids(
                    products.signatures().types(),
                    || serde_json::to_vec(value).unwrap(),
                    format!("{value:?}"),
                ),
            )
        };
        let mut lowered = 0;
        for path in &definitions {
            assert_eq!(
                actual_graph.revision(path).unwrap(),
                expected_graph.revision(path).unwrap(),
                "{path}"
            );
            let Ok(expected_bytes) = reference.definition_decode_bytes(path) else {
                continue;
            };
            let actual_bytes = prechecked.definition_decode_bytes(path).unwrap();
            let body_budget = ProcessBudget::new(expected_bytes + actual_bytes);
            let actual_allowance = body_budget.reserve(actual_bytes).unwrap();
            let expected_allowance = body_budget.reserve(expected_bytes).unwrap();
            let actual = prechecked.definition(path, &actual_allowance).unwrap();
            let expected = reference.definition(path, &expected_allowance).unwrap();
            let actual_bytes = prechecked.source_product_bytes(path).unwrap().unwrap();
            let expected_bytes = reference.source_product_bytes(path).unwrap().unwrap();
            let source_budget = ProcessBudget::new(actual_bytes + expected_bytes);
            let actual_source = prechecked
                .source_products(path, source_budget.reserve(actual_bytes).unwrap())
                .unwrap()
                .unwrap();
            let expected_source = reference
                .source_products(path, source_budget.reserve(expected_bytes).unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(actual_source.audit(), expected_source.audit(), "{path}");
            assert_eq!(
                render(&prechecked, &actual),
                render(&reference, &expected),
                "{path}"
            );
            let actual_bytes = prechecked.dependency_decode_bytes(path).unwrap();
            let expected_bytes = reference.dependency_decode_bytes(path).unwrap();
            let dependency_budget = ProcessBudget::new(actual_bytes + expected_bytes);
            let actual_dependencies = prechecked
                .dependencies(path, dependency_budget.reserve(actual_bytes).unwrap())
                .unwrap();
            let expected_dependencies = reference
                .dependencies(path, dependency_budget.reserve(expected_bytes).unwrap())
                .unwrap();
            assert_eq!(
                actual_dependencies.dependencies(),
                expected_dependencies.dependencies(),
                "{path}"
            );
            lowered += 1;
        }
        assert!(lowered > 100, "{lowered}");
        drop(actual_graph);
        drop(expected_graph);
        drop(prechecked);
        drop(reference);
        // The immutable native source request must complete before zero-work
        // inspection. The independent raw-import consumer below retains the
        // original snapshot/materialization/constant and V acceptance boundary.
        for requests in [&mut warm_requests, &mut cold_requests] {
            let output = requests
                .execute(&client, CompileOperation::NativeSources { roots: vec![] })
                .unwrap();
            assert!(matches!(&output.result, CompileResult::NativeSources(_)));
        }
        let mut native_prechecked = load(&client, Some(&products));
        let mut native_reference = load(&client, None);
        native_prechecked.check(LoweringPolicy::Reachable).unwrap();
        native_reference.check(LoweringPolicy::Reachable).unwrap();
        let actual = native_prechecked.engine.export_native_input().unwrap();
        let expected = native_reference.engine.export_native_input().unwrap();
        assert_eq!(actual.audits, expected.audits);
        assert_eq!(
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.materialized)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.materialized)
            )
        );
        assert_eq!(
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.constants)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.constants)
            )
        );
        native_prechecked
            .engine
            .import_native_input(actual)
            .unwrap();
        let queries = definitions
            .iter()
            .flat_map(|path| {
                [QueryKind::Check, QueryKind::Modes].map(|phase| (path.clone(), phase))
            })
            .collect::<Vec<_>>();
        for (path, phase, count) in warm_requests.recompute_counts(&queries).unwrap() {
            assert_eq!(count, 0, "{path}: {phase:?}");
        }
    });
}
