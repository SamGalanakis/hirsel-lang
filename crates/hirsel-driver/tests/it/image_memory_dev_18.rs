//! Image preparation clones read evidence into several phase/memo holders.
use hirsel_engine::*;
use hirsel_syntax::{DefPath, ItemPath};

#[test]
fn phase_and_memo_clones_share_resolver_read_storage() {
    with_compiler_stack(|| {
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine
            .sources
            .add("reads.hir", "fn main() -> u32 { absent() }\n".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let path = DefPath(ItemPath {
            package: default_context().package,
            modules: vec![],
            name: "main".into(),
        });
        assert!(matches!(
            engine.resolve_definition(&path),
            Err(QueryError::Diagnostics(_))
        ));
        let snapshot = engine.resolver_snapshot(&path, QueryKind::Resolve).unwrap();
        let cloned = snapshot.clone();
        assert!(!snapshot.reads.is_empty());
        assert_eq!(
            snapshot.reads.as_ptr(),
            cloned.reads.as_ptr(),
            "phase clone copied resolver evidence"
        );
        let detail = engine.dependency_reads(&path, QueryKind::Resolve).unwrap();
        let cloned = detail.clone();
        assert_eq!(
            detail.resolver.as_ptr(),
            cloned.resolver.as_ptr(),
            "memo clone copied resolver evidence"
        );
        assert_eq!(detail.resolver.as_slice(), snapshot.reads.as_slice());
    });
}
