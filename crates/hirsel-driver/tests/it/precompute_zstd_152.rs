use hirsel_driver::{COMPILER, CompilerRequestsExt};
use hirsel_engine::{
    CompileConfig, DefinitionQueries, QueryKind, store::DirStore, with_compiler_stack,
};
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
        let mut warm = COMPILER
            .with_config(CompileConfig {
                store: Arc::new(
                    DirStore::new(dir.path().join("reader")).with_read_only_root(products),
                ),
                jobs: hirsel_intern::jobs::Jobs::ONE,
                ..Default::default()
            })
            .load_program(&manifest)
            .unwrap();
        let mut cold = COMPILER
            .with_config(CompileConfig {
                store: Arc::new(hirsel_engine::store::NoneStore),
                jobs: hirsel_intern::jobs::Jobs::ONE,
                ..Default::default()
            })
            .load_program(&manifest)
            .unwrap();
        let actual_hash = warm.engine.definition_hash(&path).unwrap();
        let expected_hash = cold.engine.definition_hash(&path).unwrap();
        let actual = warm.engine.lower_to_tir(&path).unwrap();
        let expected = cold.engine.lower_to_tir(&path).unwrap();
        let render = |program: &hirsel_engine::LoadedProgram, value: &hirsel_tir::Definition| {
            canonical_debug::placed(
                &program.engine.sources,
                canonical_debug::canonical_ids(
                    program.engine.types(),
                    || serde_json::to_vec(value).unwrap(),
                    format!("{value:?}"),
                ),
            )
        };
        let actual_text = render(&warm, &actual.value);
        let expected_text = render(&cold, &expected.value);
        assert_eq!(actual_text, expected_text);
        assert_eq!(actual.dependencies, expected.dependencies);
        assert_eq!(actual_hash, expected_hash);
        assert_eq!(expected_hash, cold.engine.definition_hash(&path).unwrap());
        for phase in [QueryKind::Check, QueryKind::Modes] {
            assert_eq!(warm.engine.recompute_count(&path, phase), 0);
        }
    });
}
