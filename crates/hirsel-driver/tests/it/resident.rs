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
