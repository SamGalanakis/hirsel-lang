//! Released query records must not keep the resolver's body-sized read logs alive.
use crate::image_fixture::*;
use hirsel_driver::CompilerRequestsExt;
use hirsel_engine::*;
use hirsel_syntax::{DefPath, ItemPath};

#[test]
fn batch_release_releases_resolution_and_later_demand_recomputes_it() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("lib.hir", "pub fn value()->u32{7u32}\n");
        let library = f.0.join("lib.json");
        let manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": "lib", "version": "1"},
            "sources": [{"path": "lib.hir", "module": ["lib"]}],
            "dependencies": [],
        });
        std::fs::write(&library, manifest.to_string()).unwrap();
        let client = f.program(
            "client",
            "pub fn main()->u32{lib.value()}\n",
            &library,
            "lib",
        );
        let path = DefPath(ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "value".into(),
        });
        let mut producer = load(&client, None);
        let before = producer
            .engine
            .resolve_definition(&path)
            .unwrap()
            .value
            .clone();
        assert!(
            producer
                .engine
                .memo_reads(&path, QueryKind::Resolve)
                .is_some()
        );

        producer.engine.set_retention(memo::Retention::Batch);
        assert!(
            producer
                .engine
                .publish_definitions(
                    vec![path.clone()],
                    hirsel_intern::jobs::Jobs::ONE,
                    memo::Retention::Batch
                )
                .iter()
                .all(|outcome| outcome.result.is_ok())
        );
        assert!(
            producer
                .engine
                .memo_reads(&path, QueryKind::Resolve)
                .is_none()
        );
        assert_eq!(
            producer.engine.resolve_definition(&path).unwrap().value,
            before
        );

        let products = f.0.join("lib.products");
        hirsel_driver::COMPILER
            .precompute_package(&library, &products)
            .unwrap();
        let mut cached = load(&client, Some(&products));
        let mut fresh = load(&client, None);
        assert_eq!(
            cached.check(LoweringPolicy::Comptime).unwrap().diagnostics,
            fresh.check(LoweringPolicy::Comptime).unwrap().diagnostics
        );
    });
}
