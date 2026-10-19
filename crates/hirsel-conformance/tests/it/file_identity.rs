//! The archived unlink disagreement and its controls use the native agreement
//! classifier, as well as real Io recording and exact runtime replay.
use hirsel_conformance::{agree, native};
use hirsel_driver::native::{Outcome, Toolchain, smoke_matrix};
use std::path::Path;

#[test]
fn opened_file_identity_agrees_natively_and_replays_real_io_for_every_control() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("file-identity");
        std::fs::create_dir_all(&dir).unwrap();
        let matrix = smoke_matrix();
        let tools = Toolchain::for_matrix(&dir.join("runtime"), &matrix).unwrap();
        let native = agree::Native::new(
            matrix.clone(),
            tools,
            dir.join("agreement"),
            Default::default(),
        );
        for control in [
            "live", "unlink", "replace", "recreate", "atomic", "alias", "test", "archived",
        ] {
            let id = format!("PROV-2-narrow-current-{control}-run");
            let source =
                std::fs::read_to_string(root.join("conformance/PROV").join(format!("{id}.hir")))
                    .unwrap();
            let classification =
                agree::classify_native(&source, agree::Execution::Entry, Some((&native, &matrix)));
            assert_eq!(classification, agree::Classification::Valid, "{id}");
            std::fs::create_dir_all(dir.join(&id)).unwrap();
            let replay = native::replay_source(&id, &source, &native.toolchain, &dir.join(&id));
            assert!(replay.snapshot.is_none(), "{id}: {:?}", replay.snapshot);
            assert_eq!(replay.outcomes.len(), 1);
            for (_, _, outcome) in replay.outcomes {
                assert!(matches!(outcome, Outcome::Agree), "{id}: {outcome:?}");
            }
        }
        let stats = native.stats.lock().unwrap();
        assert_eq!(stats.agreed, 8 * matrix.len());
        assert!(stats.unavailable.is_empty(), "{:?}", stats.unavailable);
    });
}
