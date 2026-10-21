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
fn deferred_snapshot_extensions_preserve_unconsumed_seed_receipts() {
    with_compiler_stack(|| {
        use hirsel_intern::{
            jobs::Jobs,
            work::{self, Counter},
        };
        let source = "fn a() -> u8 { 1u8 }\nfn b() -> u8 { 2u8 }\n";
        let mut writer = engine(Arc::new(store::NoneStore));
        let file = writer.sources.add("resident.hir", source.into()).unwrap();
        writer.parse_file(file, &default_context()).unwrap();
        let seed = writer.signature_snapshot().seed();
        drop(writer);
        for jobs in [1, 2, 8] {
            let mut reader = engine(Arc::new(store::NoneStore));
            let file = reader.sources.add("resident.hir", source.into()).unwrap();
            reader.parse_file(file, &default_context()).unwrap();
            reader.seed_signature_verdicts(&seed);
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
                "only a changed view is checked; unused successful seed receipts survive deferred epochs"
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
        let before: Vec<_> = [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes]
            .into_iter()
            .map(|kind| {
                session
                    .program()
                    .unwrap()
                    .engine
                    .recompute_count(&caller, kind)
            })
            .collect();
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
        for (kind, before) in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes]
            .into_iter()
            .zip(before)
        {
            assert_eq!(
                session
                    .program()
                    .unwrap()
                    .engine
                    .recompute_count(&caller, kind),
                before
            );
        }
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
    struct Sequence(Arc<dyn LoweringPhase>, bool);
    impl LoweringPhase for Sequence {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
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
            adapters.lowering = Some(Arc::new(Sequence(
                adapters.lowering.take().unwrap(),
                stopped,
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
            .load_program_source_with_images(&source, None, &StdImages::none())
            .unwrap();
        let mut resident = COMPILER
            .with_retention(memo::Retention::Resident)
            .load_program_source_with_images(&source, None, &StdImages::none())
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
                .load_program_source_with_images(&source, Some(&edited), &StdImages::none())
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
        assert_eq!(session.program().unwrap().engine.hold_bytes(), 0);
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
