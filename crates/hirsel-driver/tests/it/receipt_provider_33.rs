//! Default and simulated original-source providers use actual V-owned bodies.
use hirsel_driver::{compiler_adapters, native::receipt_source::provider::*};
use hirsel_engine::*;
use hirsel_rc::instance::{Actuals, key_of};
use hirsel_rc::sealed_source::SourceError;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{SignatureRef, TypeInterner, TypeMemo};
use std::sync::Arc;

struct Simulated {
    types: Arc<TypeInterner>,
    current: Current,
    signature: SignatureRef,
    original: Option<Arc<NativeOriginalBody>>,
}
impl Provider for Simulated {
    fn types(&self) -> &TypeInterner {
        &self.types
    }
    fn current(&self, _: &DefPath) -> Result<Current, SourceError> {
        Ok(self.current)
    }
    fn signature(&self, _: &DefPath) -> Result<SignatureRef, SourceError> {
        Ok(self.signature.clone())
    }
    fn original(&self, _: &DefPath) -> Result<Arc<NativeOriginalBody>, SourceError> {
        self.original
            .clone()
            .ok_or(SourceError::Unsupported("simulated unavailable source"))
    }
}

#[test]
fn receipt_provider_default_and_simulated_current_source_agree_and_stale_rows_refuse() {
    with_compiler_stack(|| {
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine
            .sources
            .add("receipt-provider.hir", "pub fn main()->u64 {7u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let path = entry_path(&default_context());
        let reader = engine.native_original_reader(&[path.clone()]).unwrap();
        let front = Front::new(&reader);
        let original = front.original(&path).unwrap();
        let base = key_of(
            engine.types(),
            &path,
            original.definition().entry,
            &Actuals::none(),
            None,
            None,
        )
        .unwrap();
        let actual = named(&front, &base, &Actuals::none(), &TypeMemo::new()).unwrap();
        let mut simulated = Simulated {
            types: engine.types().clone(),
            current: front.current(&path).unwrap(),
            signature: front.signature(&path).unwrap(),
            original: Some(original),
        };
        let observed = named(&simulated, &base, &Actuals::none(), &TypeMemo::new()).unwrap();
        assert_eq!(actual.entry(), observed.entry());
        assert_eq!(actual.demand().projection(), observed.demand().projection());
        for field in 0..3 {
            let saved = simulated.current;
            let other = ContentHash::of_bytes(b"another current source observation");
            match field {
                0 => simulated.current.source = other,
                1 => simulated.current.revision = other,
                2 => simulated.current.signature = other,
                _ => unreachable!(),
            }
            assert!(matches!(
                named(&simulated, &base, &Actuals::none(), &TypeMemo::new()),
                Err(SourceError::Malformed(_))
            ));
            simulated.current = saved;
        }
        simulated.original = None;
        assert!(matches!(
            named(&simulated, &base, &Actuals::none(), &TypeMemo::new()),
            Err(SourceError::Unsupported(_))
        ));
        let mut missing = path;
        missing.0.name = "not_selected".into();
        assert!(matches!(
            front.current(&missing),
            Err(SourceError::Unsupported(_))
        ));
    });
}
