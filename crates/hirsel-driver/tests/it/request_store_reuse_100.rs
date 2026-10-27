use hirsel_driver::session::Session;
use hirsel_engine::{CheckCompletion, CompileConfig, QueryKind, store::DirStore};
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use std::sync::Arc;

#[test]
fn directory_request_products_reuse_unchanged_bodies_across_sessions_and_edits() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            &source,
            "fn stable()->u8{1u8}\npub fn main()->u8{stable()}\n",
        )
        .unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"reuse","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let config = CompileConfig {
            store: Arc::new(DirStore::new(directory.path().join("results"))),
            jobs: hirsel_intern::jobs::Jobs::ONE,
            hold_bytes: 0,
            ..Default::default()
        };
        let path = |name: &str| {
            DefPath(ItemPath {
                package: PackageId {
                    name: "reuse".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: name.into(),
            })
        };
        let queries = ["stable", "main"]
            .into_iter()
            .flat_map(|name| {
                [QueryKind::Check, QueryKind::Modes]
                    .into_iter()
                    .map(move |kind| (path(name), kind))
            })
            .collect::<Vec<_>>();
        let check = |session: &mut Session| {
            let checked = session.check().unwrap();
            assert!(checked.diagnostics.is_empty());
            assert_eq!(checked.completion, CheckCompletion::Complete);
        };
        let mut seed = Session::open_configured(&manifest, config.clone());
        check(&mut seed);
        assert!(
            seed.recompute_counts(queries.clone())
                .unwrap()
                .iter()
                .all(|(_, _, count)| *count == 1)
        );
        drop(seed);
        let mut fresh = Session::open_configured(&manifest, config);
        check(&mut fresh);
        assert!(
            fresh
                .recompute_counts(queries.clone())
                .unwrap()
                .iter()
                .all(|(_, _, count)| *count == 0)
        );
        std::fs::write(
            &source,
            "fn stable()->u8{1u8}\npub fn main()->u8{stable()+1u8}\n",
        )
        .unwrap();
        check(&mut fresh);
        for (definition, _, count) in fresh.recompute_counts(queries).unwrap() {
            assert_eq!(count, u64::from(definition == path("main")), "{definition}");
        }
    });
}
