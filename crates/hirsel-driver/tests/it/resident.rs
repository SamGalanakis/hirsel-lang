use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::Signature;
use std::sync::Arc;

fn engine(store: Arc<dyn store::ResultStore>) -> QueryEngine {
    QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store)
}
fn checked(engine: &mut QueryEngine, text: &str) -> CheckOutput {
    let file = engine.sources.add("resident.hir", text.into()).unwrap();
    check_progress(engine, file, &default_context()).unwrap()
}
fn path(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
fn diagnostics(output: &CheckOutput) -> Vec<String> {
    output
        .diagnostics
        .iter()
        .map(|d| d.render_human(&output.sources))
        .collect()
}
#[test]
fn persistent_modes_survive_a_process_and_callee_body_edits_but_read_signatures_invalidate() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let initial = "fn a(x:u8) -> u8 { x }\nfn b() -> u8 { a(1u8) }\n";
        let mut first = engine(store.clone());
        assert!(checked(&mut first, initial).diagnostics.is_empty());
        assert!(!store.is_empty());
        drop(first);
        let mut fresh = engine(store.clone());
        assert!(checked(&mut fresh, initial).diagnostics.is_empty());
        assert_eq!(fresh.recompute_count(&path("b"), QueryKind::Check), 0);
        assert_eq!(fresh.recompute_count(&path("b"), QueryKind::Modes), 0);
        let mut body_edit = engine(store.clone());
        assert!(
            checked(&mut body_edit, &initial.replace("{ x }", "{ x + 1u8 }"))
                .diagnostics
                .is_empty()
        );
        assert_eq!(body_edit.recompute_count(&path("b"), QueryKind::Check), 0);
        let changed = initial.replace("a(x:u8) -> u8", "a(x:u32) -> u32");
        let mut signature_edit = engine(store);
        let actual = checked(&mut signature_edit, &changed);
        let mut cold = engine(Arc::new(store::NoneStore));
        assert!(!actual.diagnostics.is_empty());
        assert_eq!(
            diagnostics(&actual),
            diagnostics(&checked(&mut cold, &changed))
        );
        assert!(signature_edit.recompute_count(&path("b"), QueryKind::Check) > 0);
    });
}
#[test]
fn session_rechecks_edits_and_recovers_from_invalid_sources() {
    with_compiler_stack(|| {
        let f = crate::image_fixture::Fixture::new();
        let source = f.0.join("main.hir");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let mut session = session::Session::open(&source, Verify::On);
        assert!(session.check().unwrap().diagnostics.is_empty());
        assert!(session.check().unwrap().diagnostics.is_empty());
        assert_eq!(session.stats().loads, 1);
        assert_eq!(session.stats().revision, 0);
        for text in [
            "fn main() { missing }\n",
            "fn main() {}\n",
            "fn main(",
            "fn main() {}\n",
        ] {
            std::fs::write(&source, text).unwrap();
            let actual = session.check();
            let cold = check_path(&source);
            match (actual, cold) {
                (Ok(a), Ok(b)) => assert_eq!(diagnostics(&a), diagnostics(&b)),
                (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
                (a, b) => panic!("warm/cold mismatch: {a:?} {b:?}"),
            }
        }
        assert_eq!(session.stats().loads, 1);
        assert_eq!(session.stats().revision, 4);
    });
}

#[test]
fn persistent_reads_cover_lookups_private_shapes_constants_and_moves() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let base = "type Hidden { value:u8 }\nfn get(x:Hidden) -> u8 { x.value }\nfn helper() -> u8 { 1u8 }\nconst N:u8 = comptime helper()\nfn value() -> u8 { N }\n";
        let versions = [
            base.to_owned(),
            base.replace("1u8", "2u8"),
            base.replace("value:u8", "value:u32"),
            format!("fn before() {{}}\n\n{base}"),
            format!("{base}\nfn value() -> u8 {{ 1u8 }}\n"),
            base.replace("x.value", "missing"),
            format!("{base}\nfn missing() {{}}\n"),
            base.to_owned(),
        ];
        for source in versions {
            let mut actual = engine(store.clone());
            let mut cold = engine(Arc::new(store::NoneStore));
            assert_eq!(
                diagnostics(&checked(&mut actual, &source)),
                diagnostics(&checked(&mut cold, &source)),
                "{source}"
            );
        }
    });
}

#[test]
fn verify_store_compares_canonical_products_across_new_interners() {
    with_compiler_stack(|| {
        let source = "fn a(x:u8) -> u8 { x + 1u8 }\nfn main() -> u8 { a(2u8) }\n";
        let store = Arc::new(store::MemStore::default());
        let mut first = engine(store.clone());
        assert!(checked(&mut first, source).diagnostics.is_empty());
        first.lower_to_tir(&path("main")).unwrap();
        let mut verify = engine(store).with_verify_store(store::VerifyStore::Recompute);
        assert!(checked(&mut verify, source).diagnostics.is_empty());
        verify.lower_to_tir(&path("main")).unwrap();
        assert!(verify.recompute_count(&path("main"), QueryKind::Modes) > 0);
    });
}

struct ForgedLowering;
impl LoweringPhase for ForgedLowering {
    fn lower(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        comptime: &Comptime,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        let mut output = LowerAdapter.lower(t, m, body, comptime, signature)?;
        output.value.types = Arc::from([]);
        Ok(output)
    }
    fn lower_evaluation(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
        signature: &Signature,
        expression: hirsel_syntax::ExprId,
        result: hirsel_types::TypeId,
        constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
    ) -> Result<hirsel_tir::Definition, QueryError> {
        LowerAdapter.lower_evaluation(t, m, body, staged, signature, expression, result, constants)
    }
    fn entry_body(&self) -> hirsel_tir::BodyId {
        LowerAdapter.entry_body()
    }
}
#[test]
fn switching_verification_on_checks_memory_and_disk_lowerings_without_evidence() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let mut adapters = compiler_adapters();
        adapters.lowering = Some(Arc::new(ForgedLowering));
        let mut first =
            QueryEngine::new(default_environment(), adapters).with_result_store(store.clone());
        let source = "fn main() -> u8 { 1u8 }\n";
        first.verify = Verify::Off;
        assert!(checked(&mut first, source).diagnostics.is_empty());
        first.lower_to_tir(&path("main")).unwrap();
        first.verify = Verify::On;
        assert!(matches!(
            first.lower_to_tir(&path("main")),
            Err(QueryError::Operational(_))
        ));
        // A fresh process rejects the unverifiable disk entry, lowers afresh,
        // and validates the independently rebuilt result.
        let mut second = engine(store);
        assert!(checked(&mut second, source).diagnostics.is_empty());
        let valid = second.lower_to_tir(&path("main")).unwrap();
        assert!(!valid.value.types.is_empty());
        assert_eq!(second.recompute_count(&path("main"), QueryKind::Lower), 1);
    });
}

#[test]
fn manifest_session_preserves_the_cold_loader_syntax_error_boundary() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let manifest = fixture.0.join("hirsel.json");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        std::fs::write(
            &manifest,
            serde_json::json!({
                "schema": 1, "package": {"name":"application", "version":"1"},
                "entry": {"module":[], "name":"main"},
                "sources":[{"path":"main.hir", "module":[]}], "dependencies":[],
                "unsafe_ffi":{"link":[], "headers_checked":false, "wrappers":[]}
            })
            .to_string(),
        )
        .unwrap();
        let mut session = session::Session::open(&manifest, Verify::On);
        assert!(session.check().unwrap().diagnostics.is_empty());
        std::fs::write(&source, "fn main(").unwrap();
        assert_eq!(
            session.check().unwrap_err().to_string(),
            check_path(&manifest).unwrap_err().to_string()
        );
        std::fs::write(&source, "fn main() {}\n").unwrap();
        assert!(session.check().unwrap().diagnostics.is_empty());
    });
}

#[test]
fn frozen_frontier_restores_and_verifies_persistent_modes_with_complete_reads() {
    with_compiler_stack(|| {
        use hirsel_intern::jobs::Jobs;
        fn run(
            store: Arc<dyn store::ResultStore>,
            source: &str,
            jobs: Jobs,
            verify: bool,
        ) -> (QueryEngine, Vec<String>) {
            let mut engine = engine(store);
            if verify {
                engine = engine.with_verify_store(store::VerifyStore::Recompute);
            }
            let file = engine.sources.add("bulk-store.hir", source.into()).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let paths = parsed
                .value
                .items
                .iter()
                .map(|s| s.item.definition.clone())
                .rev()
                .collect();
            let results = engine.mode_definitions(paths, jobs);
            let mut output = Vec::new();
            for result in results {
                let diagnostics = match result.result {
                    Ok(value) => value.diagnostics.clone(),
                    Err(QueryError::Diagnostics(ds)) => ds,
                    Err(error) => panic!("{error:?}"),
                };
                for diagnostic in diagnostics {
                    output.push(format!(
                        "phase {:?}\n{}",
                        engine.diagnostic_phase(&diagnostic),
                        diagnostic.render_human(&engine.sources)
                    ));
                }
            }
            (engine, output)
        }
        for jobs in [1, 2, 8] {
            let store = Arc::new(store::MemStore::default());
            let source = "pub opaque type Hidden: plain uses {} { value:u8 }\nfn get(x:Hidden) -> u8 { x.value }\nfn a(x:u8) -> u8 { x }\nfn b() -> u8 { a(1u8) }\n";
            let (first, ds) = run(store.clone(), source, Jobs::new(jobs), false);
            assert!(ds.is_empty(), "{ds:?}");
            assert!(!store.is_empty());
            drop(first);
            for edited in [source.to_owned(), source.replace("{ x }", "{ x + 1u8 }")] {
                let (warm, ds) = run(store.clone(), &edited, Jobs::new(jobs), false);
                let (_, cold) = run(Arc::new(store::NoneStore), &edited, Jobs::new(jobs), false);
                assert_eq!(ds, cold);
                assert_eq!(warm.recompute_count(&path("b"), QueryKind::Check), 0);
                assert_eq!(warm.recompute_count(&path("b"), QueryKind::Modes), 0);
            }
            for edited in [
                source.replace("a(x:u8) -> u8", "a(x:u32) -> u32"),
                source.replace("value:u8", "value:u32"),
                source.replace("x.value", "missing"),
                source.replace("x.value", "missing") + "fn missing() {}\n",
            ] {
                let (warm, ds) = run(store.clone(), &edited, Jobs::new(jobs), false);
                let (_, cold) = run(Arc::new(store::NoneStore), &edited, Jobs::new(jobs), false);
                assert_eq!(ds, cold, "jobs {jobs}: {edited}");
                if edited.contains("a(x:u32)") {
                    assert!(warm.recompute_count(&path("b"), QueryKind::Check) > 0);
                }
                if edited.contains("value:u32") {
                    assert!(warm.recompute_count(&path("get"), QueryKind::Check) > 0);
                }
            }
            let rejected = "fn identity[T](v: T) -> T { v }\n";
            let (_, expected) = run(store.clone(), rejected, Jobs::new(jobs), false);
            assert!(!expected.is_empty());
            let (restored, actual) = run(store.clone(), rejected, Jobs::new(jobs), false);
            assert_eq!(actual, expected, "stored mode rejection phase provenance");
            assert_eq!(
                restored.recompute_count(&path("identity"), QueryKind::Modes),
                0
            );
            let (verified, ds) = run(store.clone(), source, Jobs::new(jobs), true);
            assert!(ds.is_empty(), "{ds:?}");
            assert!(verified.recompute_count(&path("b"), QueryKind::Modes) > 0);
        }
    });
}

#[test]
fn restored_mode_phases_retire_with_the_edited_or_removed_memo() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let rejected = "fn identity[T](v: T) -> T { v }\n";
        let accepted = "fn identity[T](take v: T) -> T { v }\n";
        let owner = path("identity");
        let mut first = engine(store.clone());
        let file = first
            .sources
            .add("phase-retire.hir", rejected.into())
            .unwrap();
        first.parse_file(file, &default_context()).unwrap();
        let Err(QueryError::Diagnostics(before)) = first.mode_definition(&owner) else {
            panic!("borrowed result must reject")
        };
        let before: Vec<_> = before
            .into_iter()
            .map(|d| {
                let phase = first.diagnostic_phase(&d);
                (d, phase)
            })
            .collect();
        assert!(before.iter().all(|(_, phase)| phase.is_some()));
        drop(first);
        let mut restored = engine(store);
        let file = restored
            .sources
            .add("phase-retire.hir", rejected.into())
            .unwrap();
        restored.parse_file(file, &default_context()).unwrap();
        let mut result =
            restored.mode_definitions(vec![owner.clone()], hirsel_intern::jobs::Jobs::new(2));
        let Err(QueryError::Diagnostics(errors)) = result.remove(0).result else {
            panic!("stored rejection")
        };
        assert_eq!(restored.recompute_count(&owner, QueryKind::Modes), 0);
        for ((_, phase), d) in before.iter().zip(&errors) {
            assert_eq!(*phase, restored.diagnostic_phase(d));
        }
        let file = restored
            .sources
            .add("phase-retire.hir", accepted.into())
            .unwrap();
        restored.parse_file(file, &default_context()).unwrap();
        restored.mode_definition(&owner).unwrap();
        restored.last_phase = None;
        for diagnostic in &errors {
            assert_eq!(
                restored.diagnostic_phase(diagnostic),
                None,
                "old disk phase cannot outrank current successful product"
            );
        }
        for _ in 0..2 {
            let file = restored
                .sources
                .add("phase-retire.hir", "fn replacement() {}\n".into())
                .unwrap();
            restored.parse_file(file, &default_context()).unwrap();
        }
        restored.last_phase = None;
        for diagnostic in &errors {
            assert_eq!(restored.diagnostic_phase(diagnostic), None);
        }
    });
}

#[test]
fn disabled_result_provider_skips_store_work_for_scalar_and_frozen_queries() {
    struct Off;
    impl store::ResultStore for Off {
        fn enabled(&self) -> bool {
            false
        }
        fn get(&self, _: &store::EntryKey) -> Option<Arc<[u8]>> {
            panic!("off provider read")
        }
        fn put(&self, _: &store::EntryKey, _: &[u8]) {
            panic!("off provider write")
        }
        fn name(&self) -> &'static str {
            "disabled-control@1"
        }
    }
    with_compiler_stack(|| {
        let source = "fn a(x:u8) -> u8 { x + 1u8 }\nfn main() -> u8 { a(2u8) }\n";
        let mut scalar = engine(Arc::new(Off));
        assert!(checked(&mut scalar, source).diagnostics.is_empty());
        let scalar_tir = scalar.export_tir(&path("main")).unwrap();
        for jobs in [1, 2, 8] {
            let mut frozen = engine(Arc::new(Off));
            let file = frozen.sources.add("resident.hir", source.into()).unwrap();
            frozen.parse_file(file, &default_context()).unwrap();
            let results = frozen.mode_definitions(
                vec![path("main"), path("a")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert!(results.iter().all(|r| r.result.is_ok()));
            assert_eq!(frozen.export_tir(&path("main")).unwrap(), scalar_tir);
        }
        assert!(!store::ResultStore::enabled(&store::NoneStore));
        assert!(store::ResultStore::enabled(&store::MemStore::default()));
    });
}

#[test]
fn fully_restored_frozen_frontier_does_not_validate_a_signature_snapshot() {
    with_compiler_stack(|| {
        use hirsel_intern::{
            jobs::Jobs,
            work::{self, Counter},
        };
        let store = Arc::new(store::MemStore::default());
        let source = "fn a() -> u64 { 1 }\nfn b() -> u64 { 2 }\n";
        let mut first = engine(store.clone());
        assert!(checked(&mut first, source).diagnostics.is_empty());
        drop(first);
        for jobs in [1, 2, 8] {
            let mut restored = engine(store.clone());
            let file = restored.sources.add("resident.hir", source.into()).unwrap();
            restored.parse_file(file, &default_context()).unwrap();
            let paths = vec![path("b"), path("a"), path("b")];
            let before = work::read(Counter::ValidateSignatureRows);
            let results = restored.mode_definitions(paths.clone(), Jobs::new(jobs));
            assert_eq!(results.len(), paths.len());
            for (result, path) in results.iter().zip(&paths) {
                assert_eq!(&result.definition, path);
                assert_eq!(result.phase, Some(QueryKind::Modes));
                assert!(result.result.as_ref().unwrap().diagnostics.is_empty());
                for phase in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                    assert_eq!(restored.recompute_count(path, phase), 0, "{path} {phase:?}");
                }
            }
            assert_eq!(
                work::read(Counter::ValidateSignatureRows) - before,
                0,
                "fully restored frontiers have no worker requiring a frozen snapshot"
            );
            let before = work::read(Counter::ValidateSignatureRows);
            restored.mode_definitions(paths, Jobs::new(jobs));
            assert_eq!(work::read(Counter::ValidateSignatureRows) - before, 0);
        }
    });
}
