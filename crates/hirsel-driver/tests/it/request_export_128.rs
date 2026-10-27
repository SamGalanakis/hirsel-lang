use hirsel_driver::{requests::*, session::Session, *};
use hirsel_engine::{store::MemStore, *};
use std::sync::Arc;

#[test]
fn keyed_export_keeps_its_snapshot_and_matches_the_artifact_oracle_after_an_edit() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        std::fs::write(&path, "const A:u8=comptime 7u8; pub fn main()->u8{A}\n").unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"export_snapshot","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        for jobs in [1, 2] {
            let mut session = Session::open_configured(
                &manifest,
                CompileConfig {
                    store: Arc::new(MemStore::default()),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    hold_bytes: 0,
                    ..Default::default()
                },
            );
            let first = session.request(CompileOperation::ExportTir).unwrap();
            let CompileResult::ExportTir(text) = &first.result else {
                panic!("expected export")
            };
            let verifier = COMPILER.load_program(&manifest).unwrap();
            let imported = verifier.engine.import_tir(text).unwrap();
            assert!(imported.definition(&first.entry).is_some());
            let again = session.request(CompileOperation::ExportTir).unwrap();
            let CompileResult::ExportTir(repeated) = &again.result else {
                panic!("expected export")
            };
            assert_eq!(repeated, text);
            assert_eq!(again.work.recomputations, 0);
            let oracle = session
                .request(CompileOperation::Artifacts { native: false })
                .unwrap();
            let CompileResult::Artifacts(artifacts) = &oracle.result else {
                panic!("expected artifacts")
            };
            assert_eq!(
                artifacts.get("tir").map(Vec::as_slice),
                Some(text.as_bytes())
            );
            session.apply(vec![session::FileChange {
                path: path.clone(),
                bytes: Some(b"const A:u8=comptime 8u8; pub fn main()->u8{A}\n".to_vec()),
            }]);
            let edited = session
                .request_applied(CompileOperation::ExportTir)
                .unwrap();
            let CompileResult::ExportTir(edited_text) = &edited.result else {
                panic!("expected export")
            };
            assert_ne!(edited_text, text);
            assert!(verifier.engine.import_tir(text).is_ok());
        }
    });
}
