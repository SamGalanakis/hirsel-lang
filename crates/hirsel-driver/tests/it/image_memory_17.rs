//! Spilled image records must not keep the resolver's body-sized read logs alive.
use crate::image_fixture::*;
use hirsel_engine::*;
use hirsel_syntax::{DefPath, ItemPath};

#[test]
fn image_spilling_releases_resolution_and_later_demand_recomputes_it() {
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
        let mut producer = load(&client, &StdImages::none());
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

        let bytes = producer
            .image(&package("lib"), tempfile::tempfile().unwrap())
            .unwrap();
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

        let image = StdImage::from_bytes(bytes).unwrap();
        let mut cached = load(&client, &StdImages::of(vec![image]));
        let mut fresh = load(&client, &StdImages::none());
        assert!(cached.engine.imaged(&package("lib")));
        assert_eq!(
            cached.check(LoweringPolicy::Comptime).unwrap().diagnostics,
            fresh.check(LoweringPolicy::Comptime).unwrap().diagnostics
        );
    });
}
