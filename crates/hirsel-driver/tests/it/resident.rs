#[path = "../support/canonical_tir.rs"]
mod canonical_tir;
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

fn session_representation_edit_matches_cold(opaque: bool, reads_field: bool) {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let visibility = if opaque { "opaque " } else { "" };
        let body = if reads_field { "x.level" } else { "1" };
        let parameter = if reads_field { "x:Secret" } else { "" };
        let initial = format!(
            "pub {visibility}type Secret: data uses {{}} {{ level:u64, }}\n\
             fn view({parameter})->u64{{{body}}}\npub fn main()->u64{{1}}\n"
        );
        std::fs::write(&source, &initial).unwrap();
        let config = CompileConfig {
            retention: memo::Retention::Resident,
            store: Arc::new(store::NoneStore),
            ..CompileConfig::default()
        };
        let mut session = session::Session::open_configured(&source, config.clone());
        assert!(session.check_applied().unwrap().diagnostics.is_empty());
        session.checked_products(false).unwrap();
        let mut view = session.logical_entry().unwrap().clone();
        view.0.name = "view".into();
        let before = session
            .recompute_counts(
                [QueryKind::Resolve, QueryKind::Check]
                    .map(|kind| (view.clone(), kind))
                    .to_vec(),
            )
            .unwrap();

        let edited = initial.replace("level:u64", "level:u32");
        std::fs::write(&source, &edited).unwrap();
        session.apply(vec![session::FileChange {
            path: source.clone(),
            bytes: Some(edited.into_bytes()),
        }]);
        let actual = session.check_applied().unwrap();
        assert_eq!(session.stats().loads, 1);
        assert_eq!(session.stats().replaced, 1);
        let receipt = session.checked_products(true).unwrap();
        if !reads_field {
            assert_eq!(
                session
                    .recompute_counts(
                        [QueryKind::Resolve, QueryKind::Check]
                            .map(|kind| (view.clone(), kind))
                            .to_vec()
                    )
                    .unwrap(),
                before,
                "a view that does not read the representation stays cached"
            );
        }

        let mut cold = hirsel_driver::COMPILER
            .clone()
            .with_config(config)
            .with_owned_engines()
            .load_program(&source)
            .unwrap();
        let expected = cold.check_configured().unwrap();
        let expected_receipt = cold.engine.checked_products(&cold.entry).unwrap();
        assert_eq!(diagnostics(&actual), diagnostics(&expected));
        assert_eq!(actual.completion, expected.completion);
        assert_eq!(receipt.digest, expected_receipt.digest);
        assert_eq!(receipt.canonical, expected_receipt.canonical);
        assert_eq!(
            receipt.canonical_reads(),
            expected_receipt.canonical_reads()
        );
    });
}

#[test]
fn session_opaque_representation_edit_matches_cold_including_receipt() {
    session_representation_edit_matches_cold(true, true);
}

#[test]
fn session_unread_opaque_representation_edit_keeps_check_cached() {
    session_representation_edit_matches_cold(true, false);
}

#[test]
fn session_transparent_representation_edit_matches_cold_including_receipt() {
    session_representation_edit_matches_cold(false, true);
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

fn manifest_check_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let fixture = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("main.hir"), "fn main() {}\n").unwrap();
    let manifest = fixture.path().join("hirsel.json");
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
    (fixture, manifest)
}

fn diagnostic_objects(output: &CheckOutput) -> Vec<serde_json::Value> {
    output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.to_json(&output.sources))
        .collect()
}

fn assert_manifest_check_parity(session: &mut session::Session) -> CheckOutput {
    // Use the compiler directly so this oracle cannot lease a resident session.
    let cold = hirsel_driver::COMPILER.check_path(session.entry()).unwrap();
    let actual = session.check().unwrap();
    assert_eq!(actual.completion, cold.completion);
    assert_eq!(diagnostic_objects(&actual), diagnostic_objects(&cold));
    // An unchanged check must retain the same rejection or acceptance.
    let repeated = session.check().unwrap();
    assert_eq!(repeated.completion, cold.completion);
    assert_eq!(diagnostic_objects(&repeated), diagnostic_objects(&cold));
    actual
}

#[test]
fn manifest_session_preserves_the_cold_loader_syntax_error_boundary() {
    with_compiler_stack(|| {
        let (fixture, manifest) = manifest_check_fixture();
        let source = fixture.path().join("main.hir");
        let mut session = session::Session::open(&manifest, Verify::On);
        let valid = assert_manifest_check_parity(&mut session);
        assert!(valid.diagnostics.is_empty());
        assert_eq!(valid.completion, CheckCompletion::Complete);
        std::fs::write(&source, "fn main(").unwrap();
        let invalid = assert_manifest_check_parity(&mut session);
        assert_eq!(invalid.diagnostics.len(), 1);
        let diagnostic = &invalid.diagnostics[0];
        assert_eq!(diagnostic.rule.as_str(), "LANG-1");
        assert_eq!(diagnostic.severity, hirsel_diag::Severity::Error);
        let span = invalid.sources.resolve(diagnostic.primary);
        assert_eq!(invalid.sources.file(span.file).unwrap().path, source);
        assert_eq!((span.start, span.end), (8, 8));
        assert_eq!(invalid.completion, CheckCompletion::Complete);
        // Checking exposes diagnostics, but requesting a program still cannot
        // return the old valid product while its source no longer parses.
        assert!(matches!(
            session.request_applied(hirsel_driver::requests::CompileOperation::Tir {
                definition: None
            }),
            Err(ToolError::Query {
                error: QueryError::Diagnostics(_),
                ..
            })
        ));
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let repaired = assert_manifest_check_parity(&mut session);
        assert!(repaired.diagnostics.is_empty());
        assert_eq!(repaired.completion, valid.completion);
        assert_eq!(session.stats().loads, 1);
        assert_eq!(session.stats().replaced, 2);
        let receipt = session.checked_products(true).unwrap();
        let mut fresh = hirsel_driver::COMPILER
            .load_check_package(&manifest)
            .unwrap();
        assert!(fresh.check_configured().unwrap().diagnostics.is_empty());
        let expected = fresh.engine.checked_products(&fresh.entry).unwrap();
        assert_eq!(receipt.canonical, expected.canonical);
        assert_eq!(receipt.digest, expected.digest);
        // The error output owns the invalid snapshot even after the repair.
        assert_eq!(invalid.sources.file(span.file).unwrap().text(), "fn main(");
    });
}

#[test]
fn manifest_session_missing_source_remains_an_io_error() {
    with_compiler_stack(|| {
        let (fixture, manifest) = manifest_check_fixture();
        let mut session = session::Session::open(&manifest, Verify::On);
        assert!(session.check().unwrap().diagnostics.is_empty());
        std::fs::remove_file(fixture.path().join("main.hir")).unwrap();
        for result in [
            session.check(),
            hirsel_driver::COMPILER.check_path(&manifest),
        ] {
            let Err(ToolError::Io(error)) = result else {
                panic!("missing source must remain an I/O error: {result:?}");
            };
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        }
        std::fs::write(fixture.path().join("main.hir"), "fn main() {}\n").unwrap();
        assert!(
            assert_manifest_check_parity(&mut session)
                .diagnostics
                .is_empty()
        );
    });
}

#[test]
fn manifest_session_malformed_manifest_remains_a_tool_error() {
    with_compiler_stack(|| {
        let (_fixture, manifest) = manifest_check_fixture();
        let bytes = std::fs::read(&manifest).unwrap();
        let mut session = session::Session::open(&manifest, Verify::On);
        assert!(session.check().unwrap().diagnostics.is_empty());
        std::fs::write(&manifest, "{").unwrap();
        for result in [
            session.check(),
            hirsel_driver::COMPILER.check_path(&manifest),
        ] {
            let Err(ToolError::Runtime(message)) = result else {
                panic!("malformed manifest must remain a tool error: {result:?}");
            };
            assert!(message.starts_with("package manifest:"), "{message}");
        }
        std::fs::write(&manifest, bytes).unwrap();
        assert!(
            assert_manifest_check_parity(&mut session)
                .diagnostics
                .is_empty()
        );
    });
}

#[test]
fn manifest_session_preserves_non_diagnostic_query_errors() {
    struct FailingLexer(QueryError);
    impl LexerPhase for FailingLexer {
        fn lex(
            &self,
            _: hirsel_span::FileId,
            _: &hirsel_span::SourceMap,
        ) -> Result<QueryOutput<Vec<hirsel_syntax::Token>>, QueryError> {
            Err(self.0.clone())
        }
    }
    fn unavailable() -> PhaseAdapters {
        let mut adapters = compiler_adapters();
        adapters.lexer = Some(Arc::new(FailingLexer(QueryError::NotImplemented)));
        adapters
    }
    fn disconnected() -> PhaseAdapters {
        let mut adapters = compiler_adapters();
        adapters.lexer = Some(Arc::new(FailingLexer(QueryError::Operational(
            hirsel_types::OperationalFailure {
                operation: "fixture-transport".into(),
                message: "connection lost".into(),
            },
        ))));
        adapters
    }
    with_compiler_stack(|| {
        for error in [
            QueryError::NotImplemented,
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "fixture-transport".into(),
                message: "connection lost".into(),
            }),
        ] {
            let (fixture, manifest) = manifest_check_fixture();
            let factory = if error == QueryError::NotImplemented {
                unavailable
            } else {
                disconnected
            };
            let mut session = session::Session::open_with_compiler(
                &manifest,
                hirsel_driver::COMPILER.with_phase_adapters(
                    factory,
                    [if error == QueryError::NotImplemented {
                        0x47
                    } else {
                        0x48
                    }; 32],
                ),
            );
            // Force a new parse, with the injected failure at the loader seam.
            std::fs::write(fixture.path().join("main.hir"), "fn main() { }\n").unwrap();
            let Err(ToolError::Query { error: actual, .. }) = session.check() else {
                panic!("non-diagnostic query error was flattened");
            };
            assert_eq!(actual, error);
        }
    });
}

#[test]
fn frozen_frontier_restores_and_verifies_persistent_modes_with_complete_reads() {
    persistent_frontier_control(false);
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
        fn get(&self, _: &store::EntryKey) -> Option<hirsel_engine::store::Bytes> {
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
            let mut compact = engine(Arc::new(Off));
            let file = compact.sources.add("resident.hir", source.into()).unwrap();
            compact.parse_file(file, &default_context()).unwrap();
            let results = compact.publish_definitions(
                vec![path("main"), path("a")],
                hirsel_intern::jobs::Jobs::new(jobs),
                memo::Retention::Batch,
            );
            assert!(results.iter().all(|r| r.result.is_ok()));
            assert_eq!(compact.export_tir(&path("main")).unwrap(), scalar_tir);
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

#[test]
fn cold_frozen_frontier_defers_v_until_the_validation_universe_is_requested() {
    with_compiler_stack(|| {
        use hirsel_intern::{
            jobs::Jobs,
            work::{self, Counter},
        };
        let source = "fn a(x:u8) -> u8 { x + 1u8 }\nfn b() -> u8 { a(2u8) }\n";
        for jobs in [1, 2, 8] {
            let mut frozen = engine(Arc::new(store::NoneStore));
            let file = frozen.sources.add("resident.hir", source.into()).unwrap();
            frozen.parse_file(file, &default_context()).unwrap();
            let before = work::read(Counter::ValidateSignatureRows);
            let outputs = frozen.mode_definitions(vec![path("a"), path("b")], Jobs::new(jobs));
            assert!(
                outputs
                    .iter()
                    .all(|o| o.result.as_ref().unwrap().diagnostics.is_empty())
            );
            assert_eq!(
                work::read(Counter::ValidateSignatureRows) - before,
                0,
                "ordinary P reads signatures, without consuming V's universe"
            );
            let snapshot = frozen.signature_snapshot();
            assert!(work::read(Counter::ValidateSignatureRows) > before);
            let after = work::read(Counter::ValidateSignatureRows);
            let second = frozen.signature_snapshot();
            assert!(Arc::ptr_eq(&snapshot, &second));
            assert_eq!(work::read(Counter::ValidateSignatureRows), after);
            let mut scalar = engine(Arc::new(store::NoneStore));
            assert!(checked(&mut scalar, source).diagnostics.is_empty());
            assert_eq!(
                frozen.export_tir(&path("b")).unwrap(),
                scalar.export_tir(&path("b")).unwrap()
            );
        }
    });
}

#[test]
fn deferred_snapshot_extensions_preserve_local_verifier_receipts() {
    with_compiler_stack(|| {
        use hirsel_intern::{
            jobs::Jobs,
            work::{self, Counter},
        };
        let source = "fn a() -> u8 { 1u8 }\nfn b() -> u8 { 2u8 }\n";
        for jobs in [1, 2, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut writer = engine(store.clone());
            let file = writer.sources.add("resident.hir", source.into()).unwrap();
            writer.parse_file(file, &default_context()).unwrap();
            writer.signature_snapshot();
            drop(writer);
            let mut reader = engine(store.clone());
            let file = reader.sources.add("resident.hir", source.into()).unwrap();
            reader.parse_file(file, &default_context()).unwrap();
            let before = work::read(Counter::ValidateSignatureRows);
            assert!(
                reader
                    .mode_definitions(vec![path("a"), path("b")], Jobs::new(jobs))
                    .iter()
                    .all(|o| o.result.is_ok())
            );
            let edited = source.replace("fn a() -> u8 { 1u8 }", "fn a() -> u16 { 1u16 }");
            let file = reader.sources.add("resident.hir", edited).unwrap();
            reader.parse_file(file, &default_context()).unwrap();
            assert!(
                reader
                    .mode_definitions(vec![path("a")], Jobs::new(jobs))
                    .iter()
                    .all(|o| o.result.is_ok())
            );
            assert_eq!(work::read(Counter::ValidateSignatureRows) - before, 0);
            reader.signature_snapshot();
            assert_eq!(
                work::read(Counter::ValidateSignatureRows) - before,
                1,
                "only a changed view is checked; local successful receipts survive deferred epochs"
            );
        }
    });
}
#[test]
fn front_changes_survive_early_query_recomputation_and_selective_acknowledgment() {
    with_compiler_stack(|| {
        use std::collections::BTreeSet;
        let source = "fn a() -> u8 { 1u8 }\nfn b() { let _ = a() }\nfn c() -> u64 { 7 }\n";
        let mut engine = engine(Arc::new(store::NoneStore));
        assert!(checked(&mut engine, source).diagnostics.is_empty());
        let initial = engine.frontier_changes();
        let all: Vec<_> = initial.changed.union(&initial.affected).copied().collect();
        engine.acknowledge_frontier(&initial, &all);
        let settled = engine.frontier_changes();
        assert!(
            settled.changed.is_empty() && settled.removed.is_empty() && settled.affected.is_empty()
        );

        let edited = source.replace("fn a() -> u8 { 1u8 }", "fn a() -> u8 { 2u8 }");
        let file = engine.sources.add("resident.hir", edited.clone()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let before = engine.memo_stats().touched;
        let body = engine.frontier_changes();
        assert_eq!(
            engine.memo_stats().touched,
            before,
            "collecting a body-only frontier reads no body memos"
        );
        assert_eq!(
            body.changed,
            BTreeSet::from([hirsel_syntax::DefId::intern(&path("a"))])
        );
        assert!(
            !body
                .affected
                .contains(&hirsel_syntax::DefId::intern(&path("c")))
        );

        let changed_signature = edited.replace("fn a() -> u8 { 2u8 }", "fn a() -> u16 { 2u16 }");
        let file = engine
            .sources
            .add("resident.hir", changed_signature.clone())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        engine.mode_definition(&path("b")).unwrap();
        // This query has already cleared b's graph dirty mark. Its aggregate
        // still needs the newly computed diagnostics and compact products.
        engine.acknowledge_frontier(&body, &all);
        let signature = engine.frontier_changes();
        let b = hirsel_syntax::DefId::intern(&path("b"));
        assert!(signature.affected.contains(&b));
        assert!(
            signature
                .changed
                .contains(&hirsel_syntax::DefId::intern(&path("a")))
        );
        assert!(
            !signature
                .affected
                .contains(&hirsel_syntax::DefId::intern(&path("c")))
        );
        engine.acknowledge_frontier(&signature, &[b]);
        let partial = engine.frontier_changes();
        assert!(!partial.affected.contains(&b));
        assert!(!partial.changed.is_empty());

        let file = engine
            .sources
            .add(
                "resident.hir",
                changed_signature.replace("fn b() { let _ = a() }\n", ""),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let removed = engine.frontier_changes();
        assert!(removed.removed.contains(&b));
        engine.acknowledge_frontier(&signature, &[b]);
        assert!(engine.frontier_changes().removed.contains(&b));
        engine.acknowledge_frontier(&removed, &[b]);
        assert!(!engine.frontier_changes().removed.contains(&b));
    });
}

#[test]
fn frozen_frontier_publishes_compact_inputs_before_bodies_are_released() {
    with_compiler_stack(|| {
        use hirsel_intern::jobs::Jobs;
        let source = "fn identity[T](take v:T) -> T { v }\nfn main() { let _ = identity(1u64) }\n";
        for jobs in [1, 2, 8] {
            let mut engine = engine(Arc::new(store::NoneStore));
            let file = engine.sources.add("resident.hir", source.into()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let paths = vec![path("main"), path("identity")];
            let outcomes = engine.mode_definitions(paths.clone(), Jobs::new(jobs));
            assert!(
                outcomes
                    .iter()
                    .all(|o| o.result.as_ref().unwrap().diagnostics.is_empty())
            );
            for outcome in &outcomes {
                let recorded = engine
                    .shared_memo_reads(&outcome.definition, QueryKind::Modes)
                    .unwrap();
                assert!(Arc::ptr_eq(&outcome.reads, &recorded));
                assert!(Arc::ptr_eq(
                    &outcome.summary.as_ref().unwrap().reads,
                    &recorded
                ));
            }
            for definition in &paths {
                assert_eq!(
                    engine.recompute_count(definition, QueryKind::Summary),
                    1,
                    "P publishes the final compact result directly"
                );
                let id = hirsel_syntax::DefId::intern(definition);
                let before = engine.memo_stats().recomputed.clone();
                let published = engine
                    .publish_definition(id, memo::Retention::Batch)
                    .unwrap();
                assert_eq!(published.definition, id);
                let checked = published.checked.as_ref().unwrap();
                assert_eq!(
                    engine.checked_definition_summary(id).unwrap(),
                    (checked.summary.clone(), checked.growth.clone())
                );
                assert!(engine.memo_reads(definition, QueryKind::Check).is_none());
                assert!(engine.memo_reads(definition, QueryKind::Modes).is_none());
                assert_eq!(engine.memo_stats().recomputed, before);
            }
            drop(outcomes);
            engine.mode_definition(&path("main")).unwrap();
            assert_eq!(
                engine.recompute_count(&path("main"), QueryKind::Check),
                2,
                "an explicit later body demand follows the ordinary miss path"
            );
        }
    });
}

#[test]
fn compact_front_results_preserve_diagnostics_after_batch_body_release() {
    with_compiler_stack(|| {
        use hirsel_intern::jobs::Jobs;
        for source in [
            "fn id[T](v:T) -> T { v }\n",
            "fn id[T](take v:T) -> T { v }\n",
        ] {
            let mut expected = engine(Arc::new(store::NoneStore));
            let baseline = checked(&mut expected, source);
            for jobs in [1, 2, 8] {
                let mut engine = engine(Arc::new(store::NoneStore));
                let file = engine.sources.add("resident.hir", source.into()).unwrap();
                engine.parse_file(file, &default_context()).unwrap();
                let outputs = engine.publish_definitions(
                    vec![path("id"), path("id")],
                    Jobs::new(jobs),
                    memo::Retention::Batch,
                );
                assert_eq!(outputs.len(), 2);
                for output in outputs {
                    assert!(
                        output
                            .reads
                            .iter()
                            .any(|(key, _)| *key == memo::ReadKey::Environment)
                    );
                    let (actual, summary) = match output.result {
                        Ok(output) => (output.diagnostics, Some(output.summary)),
                        Err(QueryError::Diagnostics(diagnostics)) => (diagnostics, None),
                        Err(error) => panic!("{error:?}"),
                    };
                    let rendered: Vec<_> = actual
                        .iter()
                        .map(|d| d.render_human(&engine.sources))
                        .collect();
                    assert_eq!(rendered, diagnostics(&baseline));
                    for (diagnostic, baseline_diagnostic) in
                        actual.iter().zip(&baseline.diagnostics)
                    {
                        let expected = expected.diagnostic_phase(baseline_diagnostic);
                        assert_eq!(
                            output
                                .diagnostic_phases
                                .iter()
                                .find(|(d, _)| d == diagnostic)
                                .map(|(_, phase)| phase.clone()),
                            expected
                        );
                    }
                    if let Some(summary) = summary {
                        assert_eq!(
                            summary.definition,
                            hirsel_syntax::DefId::intern(&path("id"))
                        );
                    } else {
                        assert!(!baseline.diagnostics.is_empty());
                    }
                }
                assert!(engine.memo_reads(&path("id"), QueryKind::Check).is_none());
                if baseline.diagnostics.is_empty() {
                    assert!(engine.memo_reads(&path("id"), QueryKind::Modes).is_none());
                } else {
                    assert!(
                        engine.memo_reads(&path("id"), QueryKind::Modes).is_some(),
                        "body-free failure receipts keep reverse subscriptions"
                    );
                    let checks = engine.recompute_count(&path("id"), QueryKind::Check);
                    engine
                        .checked_definition_summary(hirsel_syntax::DefId::intern(&path("id")))
                        .unwrap();
                    assert_eq!(
                        engine.recompute_count(&path("id"), QueryKind::Check),
                        checks
                    );
                }
                assert_eq!(engine.recompute_count(&path("id"), QueryKind::Modes), 1);
            }
        }
    });
}

#[test]
fn compact_program_front_refreshes_only_changed_definitions_and_signature_subscribers() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let mut text = String::from(
            "fn leaf(x:u8) -> u8 { x }\nfn caller() -> u8 { leaf(1u8) }\nfn main() {}\n",
        );
        for i in 0..512 {
            text.push_str(&format!("fn unrelated{i}() -> u8 {{ 1u8 }}\n"));
        }
        std::fs::write(&source, &text).unwrap();
        let mut session = session::Session::open(&source, Verify::On);
        assert!(session.check().unwrap().diagnostics.is_empty());
        let caller = path("caller");
        let before = session
            .recompute_counts(
                [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes]
                    .map(|kind| (caller.clone(), kind))
                    .to_vec(),
            )
            .unwrap();
        let body = text.replace("{ x }", "{ x + 1u8 }");
        std::fs::write(&source, &body).unwrap();
        let actual = session.check().unwrap();
        let touched = session.stats().touched;
        assert_eq!(
            diagnostics(&actual),
            diagnostics(&check_path(&source).unwrap())
        );
        assert!(
            touched < 100,
            "one body edit touched {touched} memos among 515 definitions"
        );
        assert_eq!(
            session
                .recompute_counts(
                    [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes]
                        .map(|kind| (caller.clone(), kind))
                        .to_vec()
                )
                .unwrap(),
            before
        );
        let signature = body.replace("leaf(x:u8) -> u8", "leaf(x:u32) -> u32");
        std::fs::write(&source, &signature).unwrap();
        let actual = session.check().unwrap();
        assert!(!actual.diagnostics.is_empty());
        assert_eq!(
            diagnostics(&actual),
            diagnostics(&check_path(&source).unwrap())
        );
        assert!(
            session.stats().touched < 200,
            "signature edit touched {} memos",
            session.stats().touched
        );
        std::fs::write(&source, &body).unwrap();
        let accepted = session.check().unwrap();
        assert!(accepted.diagnostics.is_empty());
        assert_eq!(
            diagnostics(&accepted),
            diagnostics(&check_path(&source).unwrap())
        );
        let removed = body.replace("fn caller() -> u8 { leaf(1u8) }\n", "");
        std::fs::write(&source, &removed).unwrap();
        assert_eq!(
            diagnostics(&session.check().unwrap()),
            diagnostics(&check_path(&source).unwrap())
        );
    });
}

#[test]
fn compact_program_lower_products_keep_execution_subscriptions_without_scanning_constants() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let mut text = String::from(
            "fn helper() -> u8 { 1u8 }\nconst Answer:u8 = comptime helper()\nfn main() {}\n",
        );
        for i in 0..128 {
            text.push_str(&format!("const C{i}:u8 = 2u8\n"));
        }
        std::fs::write(&source, &text).unwrap();
        let mut session = session::Session::open(&source, Verify::On);
        let first = session.check().unwrap();
        assert!(first.diagnostics.is_empty(), "{:?}", diagnostics(&first));
        for expression in ["(1u8 / 0u8)", "3u8"] {
            let edited = text.replace("{ 1u8 }", &format!("{{ {expression} }}"));
            std::fs::write(&source, edited).unwrap();
            let actual = session.check().unwrap();
            let touched = session.stats().touched;
            let cold = check_path(&source).unwrap();
            assert_eq!(diagnostics(&actual), diagnostics(&cold));
            assert_eq!(actual.completion, cold.completion);
            assert_eq!(actual.diagnostics.is_empty(), expression == "3u8");
            assert!(
                touched < 100,
                "one executed helper edit touched {touched} memos among 129 constants"
            );
        }
    });
}

#[test]
fn compact_lower_products_require_v_when_verification_is_enabled_after_reuse() {
    with_compiler_stack(|| {
        let mut adapters = compiler_adapters();
        adapters.lowering = Some(Arc::new(ForgedLowering));
        let mut forged = QueryEngine::new(default_environment(), adapters);
        forged.verify = Verify::Off;
        let text = "const N:u8 = 1u8\nfn main() {}\n";
        let file = forged.sources.add("verify.hir", text.into()).unwrap();
        assert!(
            check_progress(&mut forged, file, &default_context())
                .unwrap()
                .diagnostics
                .is_empty()
        );
        forged.verify = Verify::On;
        assert!(
            matches!(
                check_progress(&mut forged, file, &default_context()),
                Err(QueryError::Operational(_))
            ),
            "retained lower diagnostics cannot bypass V for the forged TIR"
        );
        let mut accepted = engine(Arc::new(store::NoneStore));
        assert!(checked(&mut accepted, text).diagnostics.is_empty());
    });
}

#[test]
fn compact_dead_arm_inputs_follow_edits_without_scanning_unchanged_body_memos() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let mut text = String::from(
            "type Failure = Used() | Stale()\n\nfn parse() -> Result[u8, Failure] { Error(Used()) }\nfn main() {}\n",
        );
        for i in 0..256 {
            text.push_str(&format!("fn unrelated{i}() -> u8 {{ 1u8 }}\n"));
        }
        std::fs::write(&source, &text).unwrap();
        let mut session = session::Session::open(&source, Verify::On);
        let first = session.check().unwrap();
        assert_eq!(first.diagnostics.len(), 1, "{:?}", diagnostics(&first));
        assert!(first.diagnostics[0].message.contains("`Stale`"));
        let extra = "fn extra() -> Result[u8, Failure] { Error(Stale()) }\n";
        for (edited, warning) in [
            (
                text.replace("Error(Used())", "Error(Stale())"),
                Some("Used"),
            ),
            (format!("{text}{extra}"), None),
            (text.clone(), Some("Stale")),
            (format!("{text}{extra}"), None),
            (text.replace("Stale()", "Renamed()"), Some("Renamed")),
            (text.replace("Error(Used())", "Error(missing())"), None),
            (text.clone(), Some("Stale")),
        ] {
            std::fs::write(&source, &edited).unwrap();
            let actual = session.check().unwrap();
            let touched = session.stats().touched;
            let cold = check_path(&source).unwrap();
            assert_eq!(diagnostics(&actual), diagnostics(&cold));
            assert_eq!(actual.completion, cold.completion);
            let lint: Vec<_> = actual
                .diagnostics
                .iter()
                .filter(|d| d.rule.as_str() == "LANG-8")
                .collect();
            match warning {
                Some(arm) => {
                    assert_eq!(lint.len(), 1);
                    assert!(lint[0].message.contains(&format!("`{arm}`")));
                }
                None => assert!(lint.is_empty()),
            }
            assert!(
                touched < 100,
                "one lint input edit touched {touched} memos among 259 definitions: {warning:?}"
            );
        }
    });
}

#[test]
fn program_constant_frontiers_evaluate_on_workers_and_reuse_published_values() {
    const CHILD: &str = "HIRSEL_CA_CONSTANT_FRONTIER_CHILD";
    if let Some(jobs) = std::env::var_os(CHILD) {
        let jobs: usize = jobs.to_str().unwrap().parse().unwrap();
        struct WorkerOnly {
            inner: Arc<dyn LoweringPhase>,
            parallel: bool,
            evaluations: std::sync::atomic::AtomicUsize,
        }
        impl LoweringPhase for WorkerOnly {
            fn lower(
                &self,
                t: &hirsel_types::TypeInterner,
                m: &hirsel_types::TypeMemo,
                body: &AnalyzedBody,
                comptime: &Comptime,
                signature: &Signature,
            ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
                self.inner.lower(t, m, body, comptime, signature)
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
                if self.parallel {
                    assert_eq!(
                        std::thread::current().name(),
                        Some("hirsel-job"),
                        "production S1 evaluated an ordinary constant on the caller"
                    );
                }
                self.evaluations
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.inner
                    .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
            }
            fn entry_body(&self) -> hirsel_tir::BodyId {
                self.inner.entry_body()
            }
        }
        with_compiler_stack(|| {
            let mut adapters = compiler_adapters();
            let guard = Arc::new(WorkerOnly {
                inner: adapters.lowering.take().unwrap(),
                parallel: jobs > 1,
                evaluations: std::sync::atomic::AtomicUsize::new(0),
            });
            adapters.lowering = Some(guard.clone());
            let mut candidate = QueryEngine::new(default_environment(), adapters);
            let source = "fn helper(x:u64)->u64{x+1}\nconst A:u64=comptime helper(1)\nconst B:u64=comptime helper(2)\nconst C:u64=comptime helper(3)\nfn main(){}\n";
            for (text, accepted) in [
                (source.to_string(), true),
                (source.replace("x+1", "x / 0"), false),
                (source.replace("x+1", "x+2"), true),
            ] {
                let file = candidate.sources.add("frontier.hir", text.clone()).unwrap();
                let actual = check_progress(&mut candidate, file, &default_context()).unwrap();
                let mut cold = engine(Arc::new(store::NoneStore));
                let expected = checked(&mut cold, &text);
                // The logical source name participates in rendered diagnostics.
                let render = |output: &CheckOutput| {
                    diagnostics(output)
                        .into_iter()
                        .map(|d| {
                            d.replace("frontier.hir", "source.hir")
                                .replace("resident.hir", "source.hir")
                        })
                        .collect::<Vec<_>>()
                };
                assert_eq!(render(&actual), render(&expected));
                assert_eq!(actual.diagnostics.is_empty(), accepted);
                if accepted {
                    let before = guard.evaluations.load(std::sync::atomic::Ordering::Relaxed);
                    assert!(before >= 3);
                    for name in ["A", "B", "C"] {
                        candidate.materialize_value(&path(name)).unwrap();
                    }
                    assert_eq!(
                        guard.evaluations.load(std::sync::atomic::Ordering::Relaxed),
                        before
                    );
                }
            }
        });
        return;
    }
    for jobs in [1, 2, 8] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "resident::program_constant_frontiers_evaluate_on_workers_and_reuse_published_values", "--nocapture"])
            .env(CHILD, jobs.to_string()).env("HIRSEL_JOBS", jobs.to_string()).output().unwrap();
        assert!(
            output.status.success(),
            "jobs={jobs}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn constant_frontier_keeps_earlier_lower_warnings_when_a_later_root_stops() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Sequence(Arc<dyn LoweringPhase>, bool, Arc<AtomicUsize>);
    impl LoweringPhase for Sequence {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            if signature.definition.path().0.name == "B" {
                self.2.fetch_add(1, Ordering::Relaxed);
            }
            if self.1 && signature.definition.path().0.name == "B" {
                return Err(QueryError::NotImplemented);
            }
            let mut output = self.0.lower(t, m, body, comptime, signature)?;
            if signature.definition.path().0.name == "A" {
                let mut warning = hirsel_diag::Diagnostic::error(
                    const { hirsel_diag::RuleId::of("LANG-8") },
                    body.typed.syntax.item.name_span,
                    "fixture: earlier constant warning",
                );
                warning.severity = hirsel_diag::Severity::Warning;
                output.diagnostics.push(warning);
            }
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
            if self.1 {
                assert_ne!(
                    body.typed.syntax.item.definition.0.name, "C",
                    "later staged constant evaluated after an earlier operational stop"
                );
            }
            self.0
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.0.entry_body()
        }
    }
    with_compiler_stack(|| {
        for stopped in [false, true] {
            let mut adapters = compiler_adapters();
            let lowerings = Arc::new(AtomicUsize::new(0));
            adapters.lowering = Some(Arc::new(Sequence(
                adapters.lowering.take().unwrap(),
                stopped,
                lowerings.clone(),
            )));
            let mut candidate = QueryEngine::new(default_environment(), adapters);
            let file = candidate
                .sources
                .add(
                    "warning-prefix.hir",
                    "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime 1\nconst B:u64=comptime 2\nconst C:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\n".into(),
                )
                .unwrap();
            let output = check_progress(&mut candidate, file, &default_context()).unwrap();
            assert_eq!(output.diagnostics.len(), 1, "{:?}", diagnostics(&output));
            assert_eq!(
                output.diagnostics[0].severity,
                hirsel_diag::Severity::Warning
            );
            assert_eq!(
                output.diagnostics[0].message,
                "fixture: earlier constant warning"
            );
            assert_eq!(
                output.completion,
                if stopped {
                    CheckCompletion::Incomplete {
                        phase: QueryKind::Lower,
                    }
                } else {
                    CheckCompletion::Complete
                }
            );
            // A noncacheable stop is retried without losing the earlier
            // warning or admitting the later dynamic declaration.
            let again = check_progress(&mut candidate, file, &default_context()).unwrap();
            assert_eq!(diagnostics(&again), diagnostics(&output));
            assert_eq!(again.completion, output.completion);
            assert!(lowerings.load(Ordering::Relaxed) > 0);
            assert_eq!(
                candidate.lower_adapter_calls(&path("B")),
                lowerings.load(Ordering::Relaxed) as u64,
                "every Lower invocation counts, including nonretained failures"
            );
        }
    });
}

#[test]
fn batch_publication_releases_completed_worker_bodies_before_the_next_task() {
    use std::sync::{
        Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    };
    struct ObservedModes {
        inner: Arc<dyn ModesPhase>,
        bodies: Mutex<Vec<Weak<TypedBody>>>,
        peak: AtomicUsize,
    }
    impl ModesPhase for ObservedModes {
        fn check_modes(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: Arc<TypedBody>,
            signature: &Signature,
            products: &mut PhaseProducts,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
            let output = self
                .inner
                .check_modes(t, m, body, signature, products, provider)?;
            let mut bodies = self.bodies.lock().unwrap();
            bodies.retain(|body| body.strong_count() != 0);
            bodies.push(Arc::downgrade(&output.value.typed));
            self.peak.fetch_max(bodies.len(), Ordering::Relaxed);
            Ok(output)
        }
    }
    with_compiler_stack(|| {
        for jobs in [1, 2, 8] {
            let mut adapters = compiler_adapters();
            let observed = Arc::new(ObservedModes {
                inner: adapters.modes.take().unwrap(),
                bodies: Mutex::new(vec![]),
                peak: AtomicUsize::new(0),
            });
            adapters.modes = Some(observed.clone());
            let mut candidate = QueryEngine::new(default_environment(), adapters);
            let source: String = (0..64)
                .map(|i| format!("fn task{i}()->u64{{{i}u64}}\n"))
                .collect();
            let file = candidate.sources.add("bounded.hir", source).unwrap();
            candidate.parse_file(file, &default_context()).unwrap();
            let paths: Vec<_> = (0..64).rev().map(|i| path(&format!("task{i}"))).collect();
            let outputs = candidate.publish_definitions(
                paths.clone(),
                hirsel_intern::jobs::Jobs::new(jobs),
                memo::Retention::Batch,
            );
            assert_eq!(
                outputs
                    .iter()
                    .map(|output| &output.definition)
                    .collect::<Vec<_>>(),
                paths.iter().collect::<Vec<_>>()
            );
            assert!(outputs.iter().all(|output| output.result.is_ok()));
            let peak = observed.peak.load(Ordering::Relaxed);
            assert!(
                peak <= jobs,
                "jobs={jobs}: {peak} completed mode bodies accumulated across 64 tasks"
            );
            assert!(
                observed
                    .bodies
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|body| body.strong_count() == 0)
            );
            for path in paths {
                assert_eq!(candidate.recompute_count(&path, QueryKind::Check), 1);
                assert_eq!(candidate.recompute_count(&path, QueryKind::Modes), 1);
                let before = candidate.memo_stats().recomputed.clone();
                candidate
                    .definition_summary(hirsel_syntax::DefId::intern(&path))
                    .unwrap();
                assert_eq!(candidate.memo_stats().recomputed, before);
            }
        }
    });
}

#[test]
fn compact_workers_restore_and_verify_scalar_store_bytes_with_complete_failure_phases() {
    persistent_frontier_control(true);
}

fn persistent_frontier_control(compact: bool) {
    with_compiler_stack(|| {
        use hirsel_intern::jobs::Jobs;
        fn run_case(
            store: Arc<dyn store::ResultStore>,
            source: &str,
            jobs: Jobs,
            verify: bool,
            compact: bool,
        ) -> (QueryEngine, Vec<String>) {
            let mut engine = engine(store.clone());
            if verify {
                engine = engine.with_verify_store(store::VerifyStore::Recompute);
            }
            let file = engine.sources.add("bulk-store.hir", source.into()).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let paths: Vec<_> = parsed
                .value
                .items
                .iter()
                .map(|s| s.item.definition.clone())
                .rev()
                .collect();
            if verify && compact {
                // Seed canonical scalar bytes, then demand immutable worker
                // capture to compare exactly through VerifyStore::Recompute.
                let mut scalar = super::resident::engine(store.clone());
                let file = scalar.sources.add("bulk-store.hir", source.into()).unwrap();
                scalar.parse_file(file, &default_context()).unwrap();
                for path in &paths {
                    let _ = scalar.mode_definition(path);
                }
            }
            let results: Vec<_> = if compact {
                engine
                    .publish_definitions(paths, jobs, memo::Retention::Batch)
                    .into_iter()
                    .map(|outcome| {
                        (
                            outcome.result.map(|output| output.diagnostics),
                            outcome.diagnostic_phases,
                        )
                    })
                    .collect()
            } else {
                engine
                    .mode_definitions(paths, jobs)
                    .into_iter()
                    .map(|outcome| {
                        let result = outcome.result.map(|output| output.diagnostics.clone());
                        let diagnostics = match &result {
                            Ok(ds) | Err(QueryError::Diagnostics(ds)) => &ds[..],
                            Err(_) => &[],
                        };
                        let phases = diagnostics
                            .iter()
                            .filter_map(|d| {
                                engine.diagnostic_phase(d).map(|phase| (d.clone(), phase))
                            })
                            .collect();
                        (result, phases)
                    })
                    .collect()
            };
            let mut output = Vec::new();
            for (result, phases) in results {
                let diagnostics = match result {
                    Ok(value) | Err(QueryError::Diagnostics(value)) => value,
                    Err(error) => panic!("{error:?}"),
                };
                for diagnostic in diagnostics {
                    output.push(format!(
                        "phase {:?}\n{}",
                        phases
                            .iter()
                            .find(|(d, _)| d == &diagnostic)
                            .map(|(_, phase)| phase.clone()),
                        diagnostic.render_human(&engine.sources)
                    ));
                }
            }
            (engine, output)
        }
        let run = |store: Arc<dyn store::ResultStore>, source: &str, jobs, verify| {
            run_case(store, source, jobs, verify, compact)
        };
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
            if compact {
                let (verified, actual) = run(store.clone(), rejected, Jobs::new(jobs), true);
                assert_eq!(actual, expected, "verified rejection phase provenance");
                assert_eq!(
                    verified.recompute_count(&path("identity"), QueryKind::Modes),
                    1
                );
            }
        }
    });
}

#[test]
fn production_batch_preserves_lint_and_lowering_facts_after_front_bodies_drop() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let mut text = String::from(
            "type Failure = Used() | Stale()\nfn parse()->Result[u8,Failure]{Error(Used())}\nfn helper()->u8{2u8}\nconst N:u8=comptime helper()\nfn main(){let _=N}\n",
        );
        for i in 0..64 {
            text.push_str(&format!("fn ordinary{i}()->u8{{{i}u8}}\n"));
        }
        std::fs::write(&source, &text).unwrap();
        let mut batch = COMPILER
            .with_retention(memo::Retention::Batch)
            .load_program_source(&source, None)
            .unwrap();
        let mut resident = COMPILER
            .with_retention(memo::Retention::Resident)
            .load_program_source(&source, None)
            .unwrap();
        let first = batch.check(LoweringPolicy::Comptime).unwrap();
        let expected = resident.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(diagnostics(&first), diagnostics(&expected));
        assert_eq!(first.diagnostics.len(), 1);
        assert!(first.diagnostics[0].message.contains("`Stale`"));
        for i in 0..64 {
            let owner = path(&format!("ordinary{i}"));
            assert!(batch.engine.memo_reads(&owner, QueryKind::Check).is_none());
            assert!(batch.engine.memo_reads(&owner, QueryKind::Modes).is_none());
            assert!(
                batch.engine.memo_reads(&owner, QueryKind::Lower).is_none(),
                "ordinary body unnecessarily lowered"
            );
            assert_eq!(batch.engine.recompute_count(&owner, QueryKind::Check), 1);
            assert_eq!(batch.engine.recompute_count(&owner, QueryKind::Modes), 1);
            assert!(
                resident
                    .engine
                    .memo_reads(&owner, QueryKind::Modes)
                    .is_some()
            );
        }
        let before = batch.engine.memo_stats().recomputed.clone();
        let warm = batch.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(diagnostics(&warm), diagnostics(&first));
        assert_eq!(
            batch.engine.memo_stats().recomputed,
            before,
            "unchanged compact inputs must need no body demand"
        );
        let mut last_export = None;
        for (old, new) in [
            ("Error(Used())", "Error(Stale())"),
            ("fn helper()->u8{2u8}", "fn helper()->u8{3u8}"),
        ] {
            let edited = text.replace(old, new);
            assert!(batch.replace_source(&source, edited.clone()).unwrap());
            let actual = batch.check(LoweringPolicy::Comptime).unwrap();
            let mut cold = COMPILER
                .with_retention(memo::Retention::Resident)
                .load_program_source(&source, Some(&edited))
                .unwrap();
            let expected = cold.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(diagnostics(&actual), diagnostics(&expected));
            assert_eq!(actual.completion, expected.completion);
            for i in 0..64 {
                assert_eq!(
                    batch
                        .engine
                        .recompute_count(&path(&format!("ordinary{i}")), QueryKind::Modes),
                    1,
                    "{old} -> {new}: ordinary{i}"
                );
            }
            last_export = Some(cold.engine.export_tir(&path("N")).unwrap());
        }
        let before = batch
            .engine
            .recompute_count(&path("ordinary0"), QueryKind::Modes);
        batch.engine.mode_definition(&path("ordinary0")).unwrap();
        assert_eq!(
            batch
                .engine
                .recompute_count(&path("ordinary0"), QueryKind::Modes),
            before + 1
        );
        // PS-9 export explicitly demands every body in the entry package.
        // Keep it after the retention assertions and the ordinary miss probe.
        assert_eq!(
            batch.engine.export_tir(&path("N")).unwrap(),
            last_export.unwrap()
        );
    });
}

#[test]
fn batch_warning_phase_lives_with_the_summary_and_retires_with_its_owner() {
    struct Warning(Arc<dyn ModesPhase>);
    impl ModesPhase for Warning {
        fn check_modes(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: Arc<TypedBody>,
            signature: &Signature,
            products: &mut PhaseProducts,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
            let syntax = body.syntax.clone();
            let mut output = self
                .0
                .check_modes(t, m, body, signature, products, provider)?;
            if syntax.item.definition.0.name == "warn" {
                let mut warning = hirsel_diag::Diagnostic::error(
                    const { hirsel_diag::RuleId::of("LANG-1") },
                    syntax.item.name_span,
                    "fixture mode warning",
                );
                warning.severity = hirsel_diag::Severity::Warning;
                products
                    .mode_diagnostics
                    .push((warning.clone(), "ownership".into()));
                output.diagnostics.push(warning);
            }
            Ok(output)
        }
    }
    with_compiler_stack(|| {
        let mut adapters = compiler_adapters();
        adapters.modes = Some(Arc::new(Warning(adapters.modes.take().unwrap())));
        let mut baseline_adapters = compiler_adapters();
        baseline_adapters.modes = Some(Arc::new(Warning(baseline_adapters.modes.take().unwrap())));
        let mut baseline = QueryEngine::new(default_environment(), baseline_adapters);
        let file = baseline
            .sources
            .add("warnings.hir", "fn warn(){}\n".into())
            .unwrap();
        baseline.parse_file(file, &default_context()).unwrap();
        let expected = baseline.mode_definition(&path("warn")).unwrap();
        assert_eq!(expected.diagnostics.len(), 1);
        let expected_phase = baseline.diagnostic_phase(&expected.diagnostics[0]);
        assert_eq!(expected_phase, Some("ownership".into()));
        let mut engine = QueryEngine::new(default_environment(), adapters);
        let file = engine
            .sources
            .add("warnings.hir", "fn warn(){}\n".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let outcomes = engine.publish_definitions(
            vec![path("warn")],
            hirsel_intern::jobs::Jobs::new(2),
            memo::Retention::Batch,
        );
        let diagnostic = outcomes[0].result.as_ref().unwrap().diagnostics[0].clone();
        assert_eq!(
            diagnostic.render_human(&engine.sources),
            expected.diagnostics[0].render_human(&baseline.sources)
        );
        assert_eq!(engine.diagnostic_phase(&diagnostic), expected_phase);
        assert!(engine.memo_reads(&path("warn"), QueryKind::Check).is_none());
        assert!(engine.memo_reads(&path("warn"), QueryKind::Modes).is_none());
        let file = engine
            .sources
            .add("warnings.hir", "fn quiet(){}\n".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let outcomes = engine.publish_definitions(
            vec![path("quiet")],
            hirsel_intern::jobs::Jobs::new(2),
            memo::Retention::Batch,
        );
        assert!(outcomes[0].result.as_ref().unwrap().diagnostics.is_empty());
        engine.last_phase = None;
        assert_eq!(engine.diagnostic_phase(&diagnostic), None);
    });
}

#[test]
fn configured_compilers_and_sessions_share_providers_and_preserve_edit_results() {
    use hirsel_intern::jobs::Jobs;
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let original = "fn helper()->u8{1u8}\nfn main()->u8{helper()}\n";
        std::fs::write(&source, original).unwrap();
        let store = Arc::new(store::MemStore::default());
        let config = CompileConfig {
            jobs: Jobs::new(8),
            hold_bytes: 0,
            store: store.clone(),
            lowering: LoweringPolicy::Reachable,
            ..CompileConfig::for_memory_cap(None)
        };
        let compiler = COMPILER.with_config(config.clone());
        let mut first = compiler.load_program(&source).unwrap();
        assert_eq!(first.engine.hold_bytes(), 0);
        assert!(first.check_configured().unwrap().diagnostics.is_empty());
        assert!(
            first
                .engine
                .memo_reads(&path("main"), QueryKind::Lower)
                .is_some(),
            "configured Reachable lowers an ordinary body"
        );
        assert!(!store.is_empty());
        drop(first);
        let mut warm = compiler.clone().load_program(&source).unwrap();
        assert!(warm.check_configured().unwrap().diagnostics.is_empty());
        assert_eq!(
            warm.engine.recompute_count(&path("main"), QueryKind::Check),
            0,
            "the cloned compiler shares its configured provider"
        );
        let mut session = session::Session::open_configured(&source, config);
        assert!(session.check().unwrap().diagnostics.is_empty());
        assert_eq!(session.hold_bytes(), 0);
        for text in [original.replace("helper()}", "missing()}"), original.into()] {
            std::fs::write(&source, &text).unwrap();
            let actual = session.check().unwrap();
            let mut cold = COMPILER
                .with_config(CompileConfig {
                    jobs: Jobs::ONE,
                    hold_bytes: 0,
                    ..CompileConfig::for_memory_cap(None)
                })
                .load_program(&source)
                .unwrap();
            let expected = cold.check_configured().unwrap();
            assert_eq!(diagnostics(&actual), diagnostics(&expected));
        }
    });
}

#[test]
fn batch_serial_compile_time_demands_release_bodies_after_each_root() {
    use std::sync::{Mutex, Weak};
    struct ObservedModes {
        inner: Arc<dyn ModesPhase>,
        bodies: Mutex<Vec<Weak<TypedBody>>>,
    }
    impl ModesPhase for ObservedModes {
        fn check_modes(
            &self,
            types: &hirsel_types::TypeInterner,
            memo: &hirsel_types::TypeMemo,
            body: Arc<TypedBody>,
            signature: &Signature,
            products: &mut PhaseProducts,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
            let output = self
                .inner
                .check_modes(types, memo, body, signature, products, provider)?;
            self.bodies
                .lock()
                .unwrap()
                .push(Arc::downgrade(&output.value.typed));
            Ok(output)
        }
    }
    with_compiler_stack(|| {
        let mut adapters = compiler_adapters();
        let observed = Arc::new(ObservedModes {
            inner: adapters.modes.take().unwrap(),
            bodies: Mutex::new(vec![]),
        });
        adapters.modes = Some(observed.clone());
        let mut candidate =
            QueryEngine::new(default_environment(), adapters).with_config(CompileConfig {
                jobs: hirsel_intern::jobs::Jobs::ONE,
                hold_bytes: 0,
                ..CompileConfig::for_memory_cap(None)
            });
        let source: String = (0..64)
            .map(|i| format!("fn h{i}()->u64{{{i}u64}}\nconst C{i}:u64=comptime h{i}()\n"))
            .collect();
        let file = candidate
            .sources
            .add("serial-held.hir", source.clone())
            .unwrap();
        candidate.parse_file(file, &default_context()).unwrap();
        let owners: Vec<_> = (0..64)
            .flat_map(|i| [path(&format!("h{i}")), path(&format!("C{i}"))])
            .collect();
        assert!(
            candidate
                .publish_definitions(
                    owners,
                    hirsel_intern::jobs::Jobs::ONE,
                    memo::Retention::Batch
                )
                .iter()
                .all(|outcome| outcome.result.is_ok())
        );
        assert!(
            observed
                .bodies
                .lock()
                .unwrap()
                .iter()
                .all(|body| body.strong_count() == 0)
        );
        let mut reference = engine(Arc::new(store::NoneStore));
        let file = reference.sources.add("serial-held.hir", source).unwrap();
        reference.parse_file(file, &default_context()).unwrap();
        for i in 0..64 {
            let root = path(&format!("C{i}"));
            let actual = candidate.materialize_constant(&root).unwrap();
            assert_eq!(actual, reference.materialize_constant(&root).unwrap());
            let live = observed
                .bodies
                .lock()
                .unwrap()
                .iter()
                .filter(|body| body.strong_count() != 0)
                .count();
            assert_eq!(
                live, 0,
                "root {i} left completed typed bodies in engine tables"
            );
            for name in [format!("C{i}"), format!("h{i}")] {
                for kind in [QueryKind::Check, QueryKind::Modes] {
                    assert!(
                        candidate.memo_reads(&path(&name), kind).is_none(),
                        "{name} retained {kind:?}"
                    );
                }
            }
        }
        let before = candidate.memo_stats().recomputed.clone();
        for i in 0..64 {
            assert_eq!(
                candidate
                    .materialize_constant(&path(&format!("C{i}")))
                    .unwrap(),
                hirsel_tir::Constant::Integer(i)
            );
        }
        assert_eq!(candidate.memo_stats().recomputed, before);
    });
}

#[test]
fn durable_front_workers_drain_payloads_before_accepting_another_task() {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    struct ObservedStore {
        pending: AtomicUsize,
        peak: AtomicUsize,
        parses: AtomicUsize,
        writers: Mutex<Vec<std::thread::ThreadId>>,
        summaries: AtomicUsize,
        signatures: AtomicUsize,
        types: AtomicUsize,
        home: std::thread::ThreadId,
    }
    impl store::ResultStore for ObservedStore {
        fn get(&self, _: &store::EntryKey) -> Option<hirsel_engine::store::Bytes> {
            None
        }
        fn put(&self, key: &store::EntryKey, _: &[u8]) {
            // Ordinary Parse transport precedes the semantic worker interval.
            // Every semantic query still publishes through put_query below.
            assert_eq!(
                key.2,
                QueryKind::Parse,
                "query publication must identify its product kind"
            );
            assert_eq!(std::thread::current().id(), self.home);
            self.parses.fetch_add(1, Ordering::SeqCst);
        }
        fn put_content(&self, domain: store::Domain, bytes: &[u8]) -> Option<store::Reference> {
            if domain.kind == 2 {
                self.types.fetch_add(1, Ordering::SeqCst);
            } else if [QueryKind::Summary, QueryKind::Signature]
                .iter()
                .any(|k| domain.kind == *k as u32 + 16)
            {
                assert_eq!(std::thread::current().id(), self.home);
            } else {
                assert_eq!(domain.kind, QueryKind::Modes as u32 + 16);
            }
            domain.reference(bytes)
        }
        fn put_query(&self, kind: QueryKind, _: &store::EntryKey, _: store::Action, _: &[u8]) {
            if kind == QueryKind::Signature {
                assert_eq!(std::thread::current().id(), self.home);
                self.signatures.fetch_add(1, Ordering::SeqCst);
                return;
            }
            if kind == QueryKind::Summary {
                assert_eq!(std::thread::current().id(), self.home);
                self.summaries.fetch_add(1, Ordering::SeqCst);
                return;
            }
            assert_eq!(kind, QueryKind::Modes);
            assert!(self.pending.fetch_sub(1, Ordering::SeqCst) > 0);
            self.writers
                .lock()
                .unwrap()
                .push(std::thread::current().id());
        }
        fn name(&self) -> &'static str {
            "observed@1"
        }
    }
    struct ObservedModes(Arc<dyn ModesPhase>, Arc<ObservedStore>);
    impl ModesPhase for ObservedModes {
        fn check_modes(
            &self,
            types: &hirsel_types::TypeInterner,
            memo: &hirsel_types::TypeMemo,
            body: Arc<TypedBody>,
            signature: &Signature,
            products: &mut PhaseProducts,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
            let output = self
                .0
                .check_modes(types, memo, body, signature, products, provider)?;
            let pending = self.1.pending.fetch_add(1, Ordering::SeqCst) + 1;
            self.1.peak.fetch_max(pending, Ordering::SeqCst);
            Ok(output)
        }
    }
    with_compiler_stack(|| {
        for jobs in [1, 2, 8] {
            let store = Arc::new(ObservedStore {
                pending: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                parses: AtomicUsize::new(0),
                writers: Mutex::new(vec![]),
                summaries: AtomicUsize::new(0),
                signatures: AtomicUsize::new(0),
                types: AtomicUsize::new(0),
                home: std::thread::current().id(),
            });
            let mut adapters = compiler_adapters();
            adapters.modes = Some(Arc::new(ObservedModes(
                adapters.modes.take().unwrap(),
                store.clone(),
            )));
            let mut candidate =
                QueryEngine::new(default_environment(), adapters).with_result_store(store.clone());
            let source: String = (0..64)
                .map(|i| format!("fn p{i}()->u64{{{i}u64}}\n"))
                .collect();
            let file = candidate.sources.add("pending-store.hir", source).unwrap();
            candidate.parse_file(file, &default_context()).unwrap();
            assert_eq!(store.parses.load(Ordering::SeqCst), 1);
            let owners: Vec<_> = (0..64).rev().map(|i| path(&format!("p{i}"))).collect();
            let output = candidate.publish_definitions(
                owners.clone(),
                hirsel_intern::jobs::Jobs::new(jobs),
                memo::Retention::Batch,
            );
            assert!(output.iter().all(|result| result.result.is_ok()));
            assert_eq!(
                output
                    .iter()
                    .map(|result| &result.definition)
                    .collect::<Vec<_>>(),
                owners.iter().collect::<Vec<_>>()
            );
            assert_eq!(store.pending.load(Ordering::SeqCst), 0);
            assert!(
                store.peak.load(Ordering::SeqCst) <= jobs,
                "jobs={jobs} pending={} encoded roots accumulated before store publication",
                store.peak.load(Ordering::SeqCst)
            );
            let writers = store.writers.lock().unwrap();
            assert_eq!(writers.len(), 64);
            assert_eq!(store.summaries.load(Ordering::SeqCst), 64);
            assert_eq!(store.signatures.load(Ordering::SeqCst), 64);
            assert!(store.types.load(Ordering::SeqCst) > 0);
            if jobs > 1 {
                assert!(
                    writers
                        .iter()
                        .all(|thread| *thread != std::thread::current().id()),
                    "workers must release payload buffers before their next task"
                );
            }
        }
    });
}

#[test]
fn batch_scalar_lowerings_release_evicted_tir_but_keep_complete_receipts() {
    with_compiler_stack(|| {
        let source: String = (0..64)
            .map(|i| format!("fn h{i}()->u64{{{i}u64}}\n"))
            .collect();
        let mut candidate = engine(Arc::new(store::NoneStore)).with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::ONE,
            hold_bytes: 0,
            ..CompileConfig::for_memory_cap(None)
        });
        let file = candidate
            .sources
            .add("lower-holds.hir", source.clone())
            .unwrap();
        candidate.parse_file(file, &default_context()).unwrap();
        let mut reference = engine(Arc::new(store::NoneStore));
        let file = reference.sources.add("lower-holds.hir", source).unwrap();
        reference.parse_file(file, &default_context()).unwrap();
        for i in 0..64 {
            let root = path(&format!("h{i}"));
            let actual = candidate.lower_to_tir(&root).unwrap();
            let expected = reference.lower_to_tir(&root).unwrap();
            assert_eq!(
                canonical_tir::definition(candidate.types(), &actual.value),
                canonical_tir::definition(reference.types(), &expected.value)
            );
            assert_eq!(actual.diagnostics, expected.diagnostics);
            assert_eq!(
                candidate.memo_reads(&root, QueryKind::Lower),
                reference.memo_reads(&root, QueryKind::Lower)
            );
            let reads = candidate
                .memo_reads(&root, QueryKind::Lower)
                .unwrap()
                .clone();
            let weak = Arc::downgrade(&actual.value);
            drop(actual);
            assert_eq!(
                weak.strong_count(),
                0,
                "root {i} retained lowered TIR at zero budget"
            );
            assert_eq!(candidate.memo_reads(&root, QueryKind::Lower), Some(&reads));
            let before = candidate.recompute_count(&root, QueryKind::Lower);
            let repeated = candidate.lower_to_tir(&root).unwrap();
            assert_eq!(
                canonical_tir::definition(candidate.types(), &repeated.value),
                canonical_tir::definition(reference.types(), &expected.value)
            );
            assert_eq!(
                candidate.recompute_count(&root, QueryKind::Lower),
                before + 1
            );
            drop(repeated);
        }
    });
}

#[test]
fn scalar_lower_holds_refresh_lru_and_apply_policy_changes() {
    with_compiler_stack(|| {
        let source =
            "fn h0()->u64{0u64}\nfn h1()->u64{1u64}\nfn h2()->u64{2u64}\nfn h3()->u64{3u64}\n";
        let config = |hold_bytes, retention| CompileConfig {
            hold_bytes,
            retention,
            jobs: hirsel_intern::jobs::Jobs::ONE,
            ..CompileConfig::for_memory_cap(None)
        };
        let mut candidate = engine(Arc::new(store::NoneStore))
            .with_config(config(u64::MAX, memo::Retention::Batch));
        let file = candidate
            .sources
            .add("lower-lru.hir", source.into())
            .unwrap();
        candidate.parse_file(file, &default_context()).unwrap();
        let a = candidate.lower_to_tir(&path("h0")).unwrap().value;
        let weak_a = Arc::downgrade(&a);
        drop(a);
        let b = candidate.lower_to_tir(&path("h1")).unwrap().value;
        let weak_b = Arc::downgrade(&b);
        drop(b);
        let budget = candidate.lower_hold_stats().bytes;
        candidate = candidate.with_config(config(budget, memo::Retention::Batch));
        let before = candidate.recompute_count(&path("h0"), QueryKind::Lower);
        drop(candidate.lower_to_tir(&path("h0")).unwrap());
        assert_eq!(
            candidate.recompute_count(&path("h0"), QueryKind::Lower),
            before
        );
        let c = candidate.lower_to_tir(&path("h2")).unwrap().value;
        let weak_c = Arc::downgrade(&c);
        drop(c);
        assert!(weak_a.strong_count() > 0);
        assert_eq!(weak_b.strong_count(), 0, "the unrefreshed body is LRU");
        assert!(weak_c.strong_count() > 0);
        assert!(candidate.lower_hold_stats().bytes <= budget);
        assert_eq!(candidate.lower_hold_stats().evictions, 1);
        assert_eq!(
            candidate.available_hold_bytes() + candidate.lower_hold_stats().bytes,
            budget
        );
        assert!(
            candidate
                .memo_reads(&path("h1"), QueryKind::Lower)
                .is_some()
        );
        let before = candidate.recompute_count(&path("h1"), QueryKind::Lower);
        drop(candidate.lower_to_tir(&path("h1")).unwrap());
        assert_eq!(
            candidate.recompute_count(&path("h1"), QueryKind::Lower),
            before + 1
        );
        assert_eq!(candidate.lower_hold_stats().recomputes, 1);
        candidate.set_retention(memo::Retention::Resident);
        let d = candidate.lower_to_tir(&path("h3")).unwrap().value;
        let weak_d = Arc::downgrade(&d);
        drop(d);
        assert!(weak_d.strong_count() > 0);
        candidate = candidate.with_config(config(0, memo::Retention::Batch));
        assert_eq!(candidate.lower_hold_stats().bytes, 0);
        for weak in [weak_a, weak_b, weak_c, weak_d] {
            assert_eq!(weak.strong_count(), 0);
        }
        let before = candidate.recompute_count(&path("h3"), QueryKind::Lower);
        let held_by_caller = candidate.lower_to_tir(&path("h3")).unwrap().value;
        assert_eq!(
            candidate.recompute_count(&path("h3"), QueryKind::Lower),
            before + 1
        );
        assert_eq!(candidate.lower_hold_stats().bytes, 0);
        assert_eq!(
            Arc::strong_count(&held_by_caller),
            1,
            "only in-flight ownership survives"
        );
    });
}

#[test]
fn evicted_lower_receipts_preserve_program_reuse_and_edit_invalidation() {
    with_compiler_stack(|| {
        let f = crate::image_fixture::Fixture::new();
        let source = f.0.join("main.hir");
        let initial = "fn helper()->u64{1u64}\nconst N:u64=comptime helper()\nfn main()->u64{N}\n";
        std::fs::write(&source, initial).unwrap();
        let config = CompileConfig {
            hold_bytes: 0,
            jobs: hirsel_intern::jobs::Jobs::ONE,
            ..CompileConfig::for_memory_cap(None)
        };
        let mut program = COMPILER
            .clone()
            .with_config(config.clone())
            .load_program_source(&source, None)
            .unwrap();
        let first = program.check(LoweringPolicy::Reachable).unwrap();
        assert!(first.diagnostics.is_empty());
        let before = program.engine.memo_stats().recomputed.clone();
        let again = program.check(LoweringPolicy::Reachable).unwrap();
        assert_eq!(diagnostics(&first), diagnostics(&again));
        assert_eq!(
            program.engine.memo_stats().recomputed,
            before,
            "compact lower receipts retain unchanged program results"
        );
        assert_eq!(program.engine.lower_hold_stats().bytes, 0);
        for edited in [
            initial.replace("1u64", "2u64"),
            initial
                .replace("fn helper()->u64", "fn helper()->u32")
                .replace("1u64", "1u32"),
            initial.into(),
        ] {
            assert!(program.replace_source(&source, edited.clone()).unwrap());
            let actual = program.check(LoweringPolicy::Reachable).unwrap();
            let mut cold = COMPILER
                .clone()
                .with_config(config.clone())
                .load_program_source(&source, Some(&edited))
                .unwrap();
            let expected = cold.check(LoweringPolicy::Reachable).unwrap();
            assert_eq!(diagnostics(&actual), diagnostics(&expected));
            assert_eq!(actual.completion, expected.completion);
            if actual.diagnostics.is_empty() {
                assert_eq!(
                    program.engine.export_tir(&path("main")).unwrap(),
                    cold.engine.export_tir(&path("main")).unwrap()
                );
            }
            assert_eq!(program.engine.lower_hold_stats().bytes, 0);
        }
    });
}

#[test]
fn evicted_generated_lowerings_preserve_cold_semantic_content() {
    with_compiler_stack(|| {
        let source = r#"type Point { x:u64 }
fn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String] { Ok(codegen.body("fn(p:standalone@0/Point)->u64 uses {} { p.x + 12u64 }")) }
fn sum(p:Point)->u64 uses {} = comptime gen(shape(Point))
"#;
        let mut candidate = engine(Arc::new(store::NoneStore)).with_config(CompileConfig {
            hold_bytes: 0,
            jobs: hirsel_intern::jobs::Jobs::ONE,
            ..CompileConfig::for_memory_cap(None)
        });
        let file = candidate
            .sources
            .add("generated-held.hir", source.into())
            .unwrap();
        candidate.parse_file(file, &default_context()).unwrap();
        let mut reference = engine(Arc::new(store::NoneStore));
        let file = reference
            .sources
            .add("generated-held.hir", source.into())
            .unwrap();
        reference.parse_file(file, &default_context()).unwrap();
        let root = path("sum");
        let cold = reference.semantic_program_revision(&root).unwrap();
        drop(candidate.lower_to_tir(&root).unwrap());
        for _ in 0..2 {
            assert_eq!(candidate.semantic_program_revision(&root).unwrap(), cold);
            assert_eq!(candidate.lower_hold_stats().bytes, 0);
        }
    });
}

#[test]
fn program_dynamic_lower_frontiers_use_workers_and_retain_compact_results() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct WorkerOnly {
        inner: Arc<dyn LoweringPhase>,
        parallel: bool,
        roots: AtomicUsize,
        lowerings: AtomicUsize,
    }
    impl LoweringPhase for WorkerOnly {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            if body.typed.syntax.item.definition.0.name == "VALUE" {
                self.lowerings.fetch_add(1, Ordering::Relaxed);
            }
            self.inner.lower(t, m, body, comptime, signature)
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
            if body.typed.syntax.item.definition.0.name == "VALUE" {
                if self.parallel {
                    assert_eq!(
                        std::thread::current().name(),
                        Some("hirsel-job"),
                        "dynamic production root evaluated on caller"
                    );
                }
                self.roots.fetch_add(1, Ordering::Relaxed);
            }
            self.inner
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.inner.entry_body()
        }
    }
    with_compiler_stack(|| {
        let sources = [
            "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst VALUE:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst GOOD:u64=comptime 7\n",
            "type Point{x:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(\"fn(p:standalone@0/Point)->u64 uses {} {12u64}\"))}\nfn width[T](p:T)->u64 uses {} = comptime gen(shape(T))\nconst VALUE:u64=comptime width(Point(x:1))\nconst GOOD:u64=comptime 7\n",
        ];
        for jobs in [2, 1, 8] {
            for source in sources {
                let second = source
                    .lines()
                    .find(|line| line.starts_with("const VALUE:"))
                    .unwrap()
                    .replace("VALUE:", "SECOND:");
                let source = source.replace("const GOOD:", &format!("{second}\nconst GOOD:"));
                let mut adapters = compiler_adapters();
                let guard = Arc::new(WorkerOnly {
                    inner: adapters.lowering.take().unwrap(),
                    parallel: jobs > 1,
                    roots: AtomicUsize::new(0),
                    lowerings: AtomicUsize::new(0),
                });
                adapters.lowering = Some(guard.clone());
                let mut candidate =
                    QueryEngine::new(default_environment(), adapters).with_config(CompileConfig {
                        jobs: hirsel_intern::jobs::Jobs::new(jobs),
                        hold_bytes: 0,
                        ..CompileConfig::for_memory_cap(None)
                    });
                for text in [
                    source.to_string(),
                    source.replace("comptime 7", "comptime 9"),
                ] {
                    let file = candidate
                        .sources
                        .add("dynamic-frontier.hir", text.clone())
                        .unwrap();
                    let actual = check_progress(&mut candidate, file, &default_context()).unwrap();
                    let mut reference = engine(Arc::new(store::NoneStore));
                    let reference_file =
                        reference.sources.add("dynamic-frontier.hir", text).unwrap();
                    reference
                        .parse_file(reference_file, &default_context())
                        .unwrap();
                    // Establish expected dynamic products through scalar body
                    // demands, with the same complete production phases.
                    drop(reference.lower_to_tir(&path("VALUE")).unwrap());
                    drop(reference.lower_to_tir(&path("SECOND")).unwrap());
                    drop(reference.lower_to_tir(&path("GOOD")).unwrap());
                    let expected =
                        check_progress(&mut reference, reference_file, &default_context()).unwrap();
                    assert!(
                        expected.diagnostics.is_empty(),
                        "{:?}",
                        diagnostics(&expected)
                    );
                    assert_eq!(diagnostics(&actual), diagnostics(&expected));
                    assert_eq!(actual.completion, expected.completion);
                    assert!(guard.roots.load(Ordering::Relaxed) > 0);
                    assert!(guard.lowerings.load(Ordering::Relaxed) > 0);
                    assert_eq!(
                        candidate.lower_adapter_calls(&path("VALUE")),
                        guard.lowerings.load(Ordering::Relaxed) as u64,
                        "successful Lower work must be counted independently of retention"
                    );
                    let before = candidate.memo_stats().recomputed.clone();
                    let modes_before: std::collections::BTreeMap<_, _> = candidate
                        .recomputed(QueryKind::Modes)
                        .into_iter()
                        .map(|path| {
                            let count = candidate.recompute_count(&path, QueryKind::Modes);
                            (path, count)
                        })
                        .collect();
                    let root_evaluations = guard.roots.load(Ordering::Relaxed);
                    let again = check_progress(&mut candidate, file, &default_context()).unwrap();
                    assert_eq!(diagnostics(&again), diagnostics(&actual));
                    assert_eq!(
                        candidate.memo_stats().recomputed,
                        before,
                        "unchanged compact production results must be reused; jobs={jobs}, repeated={:?}",
                        modes_before
                            .iter()
                            .filter_map(|(path, before)| {
                                let delta =
                                    candidate.recompute_count(path, QueryKind::Modes) - before;
                                (delta != 0).then_some((path, delta))
                            })
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(guard.roots.load(Ordering::Relaxed), root_evaluations);
                    assert!(
                        candidate
                            .memo_reads(&path("VALUE"), QueryKind::Lower)
                            .is_some()
                    );
                    assert_eq!(candidate.lower_hold_stats().bytes, 0);
                    assert_eq!(
                        candidate.export_tir(&path("VALUE")).unwrap(),
                        reference.export_tir(&path("VALUE")).unwrap()
                    );
                }
            }
        }
    });
}

#[test]
fn dynamic_lower_facts_without_validation_cannot_survive_verification_enablement() {
    with_compiler_stack(|| {
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime {let xs:Inline[u8,3]=[1,2,3];scaled(xs)}\n";
        for jobs in [1, 2, 8] {
            let config = CompileConfig {
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                hold_bytes: 0,
                verify: Verify::Off,
                ..CompileConfig::for_memory_cap(None)
            };
            let mut adapters = compiler_adapters();
            adapters.lowering = Some(Arc::new(ForgedLowering));
            let mut candidate =
                QueryEngine::new(default_environment(), adapters).with_config(config.clone());
            let file = candidate
                .sources
                .add("dynamic-verification.hir", source.into())
                .unwrap();
            let unchecked = check_progress(&mut candidate, file, &default_context()).unwrap();
            assert!(unchecked.diagnostics.is_empty());
            assert_eq!(unchecked.completion, CheckCompletion::Complete);
            candidate.verify = Verify::On;
            assert!(
                matches!(
                    check_progress(&mut candidate, file, &default_context()),
                    Err(QueryError::Operational(_))
                ),
                "jobs={jobs}: invalid TIR accepted after enabling V"
            );
            let mut valid = engine(Arc::new(store::NoneStore)).with_config(CompileConfig {
                verify: Verify::On,
                ..config
            });
            let file = valid
                .sources
                .add("dynamic-verification.hir", source.into())
                .unwrap();
            let accepted = check_progress(&mut valid, file, &default_context()).unwrap();
            assert!(accepted.diagnostics.is_empty());
            assert_eq!(accepted.completion, CheckCompletion::Complete);
        }
    });
}

#[test]
fn explicitly_delivered_changes_check_the_applied_snapshot_without_repolling_disk() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let initial = b"fn main() {}\n";
        std::fs::write(&source, initial).unwrap();
        let mut session = session::Session::open(&source, Verify::On);
        assert!(session.check().unwrap().diagnostics.is_empty());
        let edited = b"fn main() { missing }\n".to_vec();
        session.apply(vec![session::FileChange {
            path: source.clone(),
            bytes: Some(edited.clone()),
        }]);
        let input_work = session.input_work();
        let actual = session.check_applied().unwrap();
        assert_eq!(session.input_work(), input_work);
        assert!(!actual.diagnostics.is_empty());
        let revision = session.stats().revision;
        session.apply(vec![session::FileChange {
            path: source.clone(),
            bytes: Some(edited),
        }]);
        assert_eq!(
            session.stats().revision,
            revision,
            "duplicate delivery is idempotent"
        );
        assert_eq!(
            diagnostics(&session.check_applied().unwrap()),
            diagnostics(&actual)
        );
        assert_eq!(session.stats().touched, 0);
        // The ordinary filesystem-owning entry point still observes disk.
        assert!(session.check().unwrap().diagnostics.is_empty());
        assert_eq!(session.stats().revision, revision + 1);
    });
}

#[test]
fn accepted_ten_thousand_definition_body_edit_reports_actual_index_and_aggregate_work() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("main.hir");
        let mut text = String::from("fn main() {}\nfn leaf(x:u8)->u8 { x }\n");
        for i in 0..9_998 {
            text.push_str(&format!("fn unrelated{i}()->u8 {{ 1u8 }}\n"));
        }
        std::fs::write(&source, &text).unwrap();
        let mut session = session::Session::open(&source, Verify::On);
        assert!(session.check().unwrap().diagnostics.is_empty());
        let before = session.work_stats();
        assert!(
            before.frontier_observations < 100_000,
            "cold publication copied too many frontier notifications: {before:?}"
        );
        let edit = text.replace("{ x }", "{ x + 1u8 }");
        session.apply(vec![session::FileChange {
            path: source.clone(),
            bytes: Some(edit.clone().into_bytes()),
        }]);
        let output = session.check_applied().unwrap();
        assert!(output.diagnostics.is_empty());
        assert_eq!(output.completion, CheckCompletion::Complete);
        let touched = session.stats().touched;
        let work = session.work_stats().since(before);
        assert_eq!(work.parsed_bytes, edit.len() as u64);
        assert_eq!(work.parsed_definitions, 10_000);
        assert_eq!(work.index_occurrence_visits, 10_000);
        assert!(work.index_anchor_visits <= 10_001, "{work:?}");
        assert!(touched < 100, "{touched} memo touches");
        // GEN-5 now demands pending roots. Parsing/indexing and lint still
        // report their real whole-file/program work independently.
        assert!((1..100).contains(&work.gen5_visits), "{work:?}");
        assert_eq!(work.growth_components, 0, "equal growth output: {work:?}");
        assert!(work.lint_definition_visits >= 10_000, "{work:?}");
        eprintln!(
            "accepted10k touched={touched} work={}",
            serde_json::to_string(&work).unwrap()
        );
        std::fs::write(&source, edit).unwrap();
        let cold = check_path(&source).unwrap();
        assert_eq!(diagnostics(&output), diagnostics(&cold));
        assert_eq!(output.completion, cold.completion);
    });
}

#[test]
fn storage_root_index_follows_unexecuted_calls_across_source_edits() {
    with_compiler_stack(|| {
        let plain = "fn callback(root:List[u64])->unique List[u64]{unique root}\nfn unused(mut r:Region[List[u64]]){}\nfn main(){}\n";
        let sealed = plain.replace(
            "fn unused(mut r:Region[List[u64]]){}",
            "fn unused(mut r:Region[List[u64]]){let escaped=region.with(mut r,callback)}",
        );
        let mut warm = engine(Arc::new(store::NoneStore));
        for (source, rejects) in [
            (plain, false),
            (&sealed, true),
            (plain, false),
            (&sealed, true),
        ] {
            let actual = checked(&mut warm, source);
            let mut cold = engine(Arc::new(store::NoneStore));
            let expected = checked(&mut cold, source);
            assert_eq!(diagnostics(&actual), diagnostics(&expected));
            assert_eq!(actual.completion, expected.completion);
            assert_eq!(actual.completion, CheckCompletion::Complete);
            assert_eq!(
                actual
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "MODE-1"),
                rejects,
                "unexecuted source roots still require storage proof: {:?}",
                diagnostics(&actual)
            );
            if !rejects {
                assert!(actual.diagnostics.is_empty(), "{:?}", diagnostics(&actual));
            }
        }
    });
}

#[test]
fn first_failed_manifest_load_keeps_the_invalid_import_as_a_watched_input() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let source = fixture.0.join("nested").join("main.hir");
        std::fs::create_dir(source.parent().unwrap()).unwrap();
        let manifest = fixture.0.join("hirsel.json");
        std::fs::write(&source, "fn main(").unwrap();
        std::fs::write(
            &manifest,
            serde_json::json!({
                "schema": 1, "package": {"name":"application", "version":"1"},
                "entry": {"module":[], "name":"main"},
                "sources":[{"path":"nested/main.hir", "module":[]}], "dependencies":[],
                "unsafe_ffi":{"link":[], "headers_checked":false, "wrappers":[]}
            })
            .to_string(),
        )
        .unwrap();
        let mut session = session::Session::open(&manifest, Verify::On);
        let rejected = assert_manifest_check_parity(&mut session);
        assert!(rejected.diagnostics.iter().any(|diagnostic| {
            diagnostic.rule.as_str() == "LANG-1"
                && diagnostic.severity == hirsel_diag::Severity::Error
        }));
        assert!(session.watched().any(|input| input == source));
        assert!(
            session.poll().is_empty(),
            "a failed load must wait for an edit"
        );
        std::fs::write(&source, "fn main() {}\n").unwrap();
        assert_eq!(session.poll().len(), 1);
        assert!(session.check().unwrap().diagnostics.is_empty());
    });
}

#[test]
fn scalar_materialization_inherits_complete_mode_receipts_on_cold_and_warm_demands() {
    with_compiler_stack(|| {
        let text = "const BASE:u8=1u8\nconst A:u8=comptime BASE\n";
        for warm in [false, true] {
            let mut candidate = engine(Arc::new(store::NoneStore));
            let file = candidate.sources.add("closure.hir", text.into()).unwrap();
            candidate.parse_file(file, &default_context()).unwrap();
            if warm {
                candidate.mode_definition(&path("A")).unwrap();
            }
            candidate.materialize_value(&path("A")).unwrap();
            let mode = candidate.memo_reads(&path("A"), QueryKind::Modes).unwrap();
            assert!(
                mode.iter()
                    .any(|(read, _)| matches!(read, ReadKey::Lookup(_)))
            );
            let executed = candidate.materialization_reads(&path("A")).unwrap();
            for expected in mode.iter() {
                assert!(
                    executed.iter().any(|actual| actual == expected),
                    "warm={warm} omitted {expected:?}"
                );
            }
        }
    });
}

#[test]
fn library_session_checks_every_source_and_cannot_launch() {
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        fixture.write("first.hir", "pub fn value() -> u8 { 1u8 }\n");
        fixture.write("other.hir", "pub fn main() -> u8 { true }\n");
        let manifest = fixture.0.join("hirsel.json");
        let mut value = serde_json::json!({
            "schema": 1, "package": {"name":"library", "version":"1"},
            "sources":[{"path":"first.hir", "module":["first"]},
                       {"path":"other.hir", "module":["other"]}], "dependencies":[]
        });
        std::fs::write(&manifest, value.to_string()).unwrap();
        let mut session = session::Session::open(&manifest, Verify::On);
        let rejected = session.check().unwrap();
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "LANG-7")
        );
        fixture.write("other.hir", "pub fn main() -> u8 { 2u8 }\n");
        assert!(session.check().unwrap().diagnostics.is_empty());
        assert_eq!(session.stats().loads, 1);
        let safe = session
            .request(hirsel_driver::requests::CompileOperation::SafeCheck)
            .unwrap();
        assert!(matches!(
            &safe.result,
            hirsel_driver::requests::CompileResult::SafeCheck { reach: None, .. }
        ));
        assert!(
            session
                .request_applied(hirsel_driver::requests::CompileOperation::Tir {
                    definition: None
                })
                .err()
                .unwrap()
                .to_string()
                .contains("launch manifest requires entry")
        );
        let compiler = hirsel_driver::COMPILER;
        let (checked, entry) = compiler.check_safe_path(&manifest).unwrap();
        assert!(checked.diagnostics.is_empty());
        assert!(entry.is_none());
        value["entry"] = serde_json::json!({"module":["other"], "name":"main"});
        std::fs::write(&manifest, value.to_string()).unwrap();
        assert!(session.check().unwrap().diagnostics.is_empty());
        assert_eq!(session.logical_entry().unwrap().0.modules, ["other"]);
    });
}
