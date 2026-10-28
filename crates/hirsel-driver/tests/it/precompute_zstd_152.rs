use hirsel_driver::{COMPILER, CompilerRequestsExt, requests::*};
use hirsel_engine::{CompileConfig, QueryKind, store::DirStore, with_compiler_stack};
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use std::sync::Arc;

#[path = "../support/canonical_debug.rs"]
mod canonical_debug;

#[test]
fn precomputed_zstd_zeros_preserves_the_cold_body_and_hash() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let std_source = include_str!("../../../../stdlib/std@1/zstd.hir");
        let start = std_source.find("\nfn zeros(").unwrap();
        let end = start + std_source[start..].find("\n@noalloc").unwrap();
        std::fs::write(dir.path().join("zstd.hir"), &std_source[start..end]).unwrap();
        let library = dir.path().join("std.json");
        std::fs::write(&library, r#"{"schema":1,"package":{"name":"std","version":"1"},"sources":[{"path":"zstd.hir","module":["zstd"]}],"dependencies":[]}"#).unwrap();
        let products = dir.path().join("products");
        COMPILER.precompute_package(&library, &products).unwrap();
        let manifest = dir.path().join("app.json");
        std::fs::write(
            dir.path().join("main.hir"),
            "pub fn main()->Result[(),()]{Ok(())}\n",
        )
        .unwrap();
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"aaa","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":library,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let path = DefPath(ItemPath {
            package: PackageId {
                name: "std".into(),
                version: "1".into(),
            },
            modules: vec!["zstd".into()],
            name: "zeros".into(),
        });
        let requests = |store: Arc<dyn hirsel_engine::store::ResultStore>| {
            let config = CompileConfig {
                store,
                jobs: hirsel_intern::jobs::Jobs::ONE,
                ..Default::default()
            };
            CompilerRequests::for_host(COMPILER.with_config(config.clone()), config)
        };
        let mut warm = requests(Arc::new(
            DirStore::new(dir.path().join("reader")).with_read_only_root(products),
        ));
        let mut cold = requests(Arc::new(hirsel_engine::store::NoneStore));
        let hash = |requests: &mut CompilerRequests| {
            let output = requests
                .execute(
                    &manifest,
                    CompileOperation::Hashes {
                        package: Some(path.0.package.clone()),
                    },
                )
                .unwrap();
            let CompileResult::Hashes(hashes) = &output.result else {
                panic!("expected definition hashes")
            };
            hashes[&path]
        };
        let actual_hash = hash(&mut warm);
        let expected_hash = hash(&mut cold);
        let precompute = |requests: &mut CompilerRequests| {
            let output = requests
                .execute(&manifest, CompileOperation::Precompute)
                .unwrap();
            let CompileResult::Precomputed { sources, .. } = &output.result else {
                panic!("expected settled source products")
            };
            sources.clone()
        };
        let actual = precompute(&mut warm);
        let expected = precompute(&mut cold);
        let bound = actual.definition_decode_bytes(&path).unwrap()
            + expected.definition_decode_bytes(&path).unwrap();
        let budget = hirsel_engine::runtime::ProcessBudget::new(bound);
        let actual_allowance = budget
            .reserve(actual.definition_decode_bytes(&path).unwrap())
            .unwrap();
        let expected_allowance = budget
            .reserve(expected.definition_decode_bytes(&path).unwrap())
            .unwrap();
        let actual_body = actual.definition(&path, &actual_allowance).unwrap();
        let expected_body = expected.definition(&path, &expected_allowance).unwrap();
        let render = |products: &PrecomputedSources, value: &hirsel_tir::Definition| {
            canonical_debug::placed(
                products.sources(),
                canonical_debug::canonical_ids(
                    products.signatures().types(),
                    || serde_json::to_vec(value).unwrap(),
                    format!("{value:?}"),
                ),
            )
        };
        let actual_text = render(&actual, &actual_body);
        let expected_text = render(&expected, &expected_body);
        assert_eq!(actual_text, expected_text);
        let bound = actual.dependency_decode_bytes(&path).unwrap()
            + expected.dependency_decode_bytes(&path).unwrap();
        let budget = hirsel_engine::runtime::ProcessBudget::new(bound);
        let actual_dependencies = actual
            .dependencies(
                &path,
                budget
                    .reserve(actual.dependency_decode_bytes(&path).unwrap())
                    .unwrap(),
            )
            .unwrap();
        let expected_dependencies = expected
            .dependencies(
                &path,
                budget
                    .reserve(expected.dependency_decode_bytes(&path).unwrap())
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            actual_dependencies.dependencies(),
            expected_dependencies.dependencies()
        );
        assert_eq!(actual_hash, expected_hash);
        assert_eq!(expected_hash, hash(&mut cold));
        let counts = warm
            .recompute_counts(
                &[QueryKind::Check, QueryKind::Modes].map(|phase| (path.clone(), phase)),
            )
            .unwrap();
        for (definition, phase, count) in counts {
            assert_eq!(count, 0, "{definition}: {phase:?}");
        }
    });
}
