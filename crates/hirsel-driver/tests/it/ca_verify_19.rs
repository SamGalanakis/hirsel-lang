//! Source Lower records never authorize private evaluation artifacts.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::Signature;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct PrivateTypeMutation(Arc<AtomicUsize>);
impl LoweringPhase for PrivateTypeMutation {
    fn lower(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        comptime: &Comptime,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        LowerAdapter.lower(t, m, body, comptime, signature)
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
        let mut definition = LowerAdapter
            .lower_evaluation(t, m, body, staged, signature, expression, result, constants)?;
        self.0.fetch_add(1, Ordering::SeqCst);
        // Preserve the public signature and ordinary source Lower bytes.
        // The private initializer's real result type is now inconsistent.
        definition.bodies[definition.entry.0 as usize].result =
            t.intern(hirsel_types::Type::with_kind(hirsel_types::TypeKind::Error));
        Ok(definition)
    }
    fn entry_body(&self) -> hirsel_tir::BodyId {
        LowerAdapter.entry_body()
    }
}

fn session(
    store: Arc<dyn store::ResultStore>,
    adapters: PhaseAdapters,
    verify: Verify,
) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), adapters).with_result_store(store);
    engine.verify = verify;
    let file = engine
        .sources
        .add(
            "witness.hir",
            "const A:u8=comptime 1u8; pub fn main()->u8{A}".into(),
        )
        .unwrap();
    assert!(
        engine
            .parse_file(file, &default_context())
            .unwrap()
            .diagnostics
            .is_empty()
    );
    engine
}

#[test]
fn source_lower_record_from_verify_off_cannot_authorize_mutated_private_types() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let main = entry_path(&default_context());
        let mut seed = session(store.clone(), compiler_adapters(), Verify::Off);
        seed.lower_to_tir(&main).unwrap();
        drop(seed);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut adapters = compiler_adapters();
        adapters.lowering = Some(Arc::new(PrivateTypeMutation(calls.clone())));
        let mut current = session(store, adapters, Verify::On);
        assert!(
            current.lower_to_tir(&main).is_err(),
            "a source record must not certify private initializer types"
        );
        assert!(
            calls.load(Ordering::SeqCst) > 0,
            "VerifyOn must demand private lowering"
        );
    });
}
