use hirsel_driver::{requests::*, session::Session, *};
use hirsel_engine::{store::MemStore, *};
use hirsel_tir::TirProvider;
use std::sync::Arc;

#[test]
fn batch_entries_match_individual_launch_bindings_and_share_checked_runtime() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            directory.path().join("main.hir"),
            "const A:u8=comptime 7u8;\npub fn main()->u8{A}\npub fn alternative()->u8{A+1u8}\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("library.hir"),
            "pub fn other()->u8{9u8}\npub fn separate()->u8{11u8}\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("library.json"), r#"{"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"library.hir","module":[]}],"dependencies":[]}"#).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"batch","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":"library.json","package":{"name":"library","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let main = COMPILER.load_program(&manifest).unwrap().entry;
        let mut alternative = main.clone();
        alternative.0.name = "alternative".into();
        let mut library = main.clone();
        library.0.package.name = "library".into();
        library.0.name = "other".into();
        let mut roots = vec![main, alternative, library];
        roots.sort();
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
            let output = session
                .request(CompileOperation::TirBatch {
                    definitions: roots.clone(),
                })
                .unwrap();
            let CompileResult::TirBatch(batch) = &output.result else {
                panic!("expected batch");
            };
            let first = batch.entry(&roots[0]).unwrap();
            let second = batch.entry(&roots[1]).unwrap();
            assert!(Arc::ptr_eq(&first.checked_runtime, &second.checked_runtime));
            assert_ne!(first.revision.digest(), second.revision.digest());
            for root in &roots {
                let entry = batch.entry(root).unwrap();
                let output = session
                    .request(CompileOperation::Tir {
                        definition: Some(root.clone()),
                    })
                    .unwrap();
                let CompileResult::Tir(individual) = &output.result else {
                    panic!("expected individual launch");
                };
                assert_eq!(entry.entry, individual.entry);
                assert_eq!(entry.revision, individual.revision);
                assert_eq!(entry.code, individual.code);
                assert_eq!(entry.callees, individual.callees);
                assert_eq!(entry.entries, individual.entries);
                assert_eq!(entry.foreign, individual.foreign);
                assert_eq!(
                    entry.signature.fingerprint,
                    individual.signature.fingerprint
                );
            }
            let repeated = session
                .request(CompileOperation::TirBatch {
                    definitions: roots.clone(),
                })
                .unwrap();
            assert_eq!(repeated.work.recomputations, 0);
            drop(repeated);
            drop(session);
            assert!(first.provider().definition(&roots[0]).is_ok());
        }
    });
}
