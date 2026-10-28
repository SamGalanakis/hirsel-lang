//! A restored checked constant still owns its separately evaluated value.
use hirsel_driver::{COMPILER, session};
use hirsel_engine::{CompileConfig, LoweringPolicy, store, with_compiler_stack};
use std::sync::Arc;

#[test]
fn restored_constant_lower_preserves_complete_cold_receipt_after_unread_type_edit() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"receipt_constant","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let text = "pub opaque type Hidden: data uses {} { value:u8 }\nconst VALUE:u64=comptime 17\npub fn main()->u64{1}\n";
        std::fs::write(&source, text).unwrap();
        let config = CompileConfig {
            lowering: LoweringPolicy::Comptime,
            store: Arc::new(store::NoneStore),
            ..Default::default()
        };
        let mut warm = session::Session::open_configured(&manifest, config.clone());
        assert!(warm.check_applied().unwrap().diagnostics.is_empty());
        let initial = warm.checked_products(true).unwrap();
        assert!(
            initial
                .canonical
                .as_deref()
                .unwrap()
                .contains("ConstantSection")
        );
        let edited = text.replace("value:u8", "value:u16");
        std::fs::write(&source, &edited).unwrap();
        warm.apply(vec![session::FileChange {
            path: source.clone(),
            bytes: Some(edited.into_bytes()),
        }]);
        assert!(warm.check_applied().unwrap().diagnostics.is_empty());
        let actual = warm.checked_products(true).unwrap();
        let mut cold = COMPILER
            .with_config(config)
            .with_owned_engines()
            .load_program(&manifest)
            .unwrap();
        assert!(cold.check_configured().unwrap().diagnostics.is_empty());
        let expected = cold.engine.checked_products(&cold.entry).unwrap();
        assert_eq!(actual.canonical, expected.canonical);
        assert_eq!(actual.canonical_reads(), expected.canonical_reads());
        assert_eq!(actual.digest, expected.digest);
    });
}
