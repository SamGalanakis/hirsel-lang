use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn publications_validate_one_complete_signature_input() {
    with_compiler_stack(|| {
        let mut baseline = 0;
        for count in [1, 16, 64, 256] {
            let source = (0..count)
                .map(|i| format!("fn f{i}(a:u8)->u8 {{ a }}\n"))
                .collect::<String>();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("signatures.hir", source).unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let before =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateSignatureRows);
            for id in &parsed.value.items {
                let path = &id.item.definition;
                let output = engine.lower_to_tir(path).unwrap();
                assert!(output.diagnostics.is_empty());
            }
            let inspected =
                hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateSignatureRows)
                    - before;
            if count == 1 {
                baseline = inspected;
            }
            assert!(
                inspected <= 2 * (baseline + count),
                "{count} publications inspected {inspected} signatures (single input {baseline})"
            );
        }
    });
}

struct MalformedUnrelated(std::sync::Arc<dyn SignaturePhase>);
impl SignaturePhase for MalformedUnrelated {
    fn signature_items(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        items: &[std::sync::Arc<hirsel_syntax::ItemSyntax>],
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<hirsel_types::Signature>, QueryError> {
        let mut output = self
            .0
            .signature_items(t, m, items, declarations, products, provider)?;
        if output.value.definition.path().0.name == "poison" {
            // A malformed row must be checked even if the producer
            // incorrectly retains its original semantic fingerprint.
            let hirsel_types::SignatureKind::Function(function) = &mut output.value.kind else {
                unreachable!()
            };
            let mut row = t.row(function.result).clone();
            row.kind = hirsel_types::TypeKind::Error;
            function.result = t.intern(row);
        }
        Ok(output)
    }
    fn signature(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        syntax: std::sync::Arc<hirsel_syntax::ItemSyntax>,
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<hirsel_types::Signature>, QueryError> {
        self.signature_items(t, m, &[syntax], declarations, products, provider)
    }
}
fn poisoned_engine() -> QueryEngine {
    let mut adapters = compiler_adapters();
    adapters.signature = Some(std::sync::Arc::new(MalformedUnrelated(
        adapters.signature.take().unwrap(),
    )));
    QueryEngine::new(default_environment(), adapters)
}
fn publish(
    engine: &mut QueryEngine,
    source: &str,
    name: &str,
) -> Result<QueryOutput<hirsel_tir::DefinitionRef>, QueryError> {
    let file = engine.sources.add("signatures.hir", source.into()).unwrap();
    let parsed = engine.parse_file(file, &default_context()).unwrap();
    let path = parsed
        .value
        .items
        .iter()
        .map(|i| &i.item)
        .find(|item| item.definition.0.name == name)
        .unwrap()
        .definition
        .clone();
    engine.lower_to_tir(&path)
}
#[test]
fn new_publications_recheck_unrelated_malformed_signatures_after_edits() {
    with_compiler_stack(|| {
        let mut engine = poisoned_engine();
        publish(&mut engine, "fn seed(a:u8)->u8{a}", "seed").unwrap();
        let edited = "fn seed(a:u8)->u8{a}\nfn poison(a:u8)->u8{a}\nfn next(a:u8)->u8{a}";
        let actual = publish(&mut engine, edited, "next").unwrap_err();
        let expected = publish(&mut poisoned_engine(), edited, "next").unwrap_err();
        assert_eq!(actual, expected);
        assert!(matches!(actual, QueryError::Operational(ref failure)
            if failure.message.contains("InvalidSignature") && failure.message.contains("poison")));
        publish(
            &mut engine,
            "fn seed(a:u8)->u8{a}\nfn restored(a:u8)->u8{a}",
            "restored",
        )
        .unwrap();
    });
}
#[test]
fn unchanged_signature_inputs_retain_their_proof_after_body_edits() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        publish(&mut engine, "fn f(a:u8)->u8{a}", "f").unwrap();
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateSignatureRows);
        publish(&mut engine, "fn f(a:u8)->u8{1u8}", "f").unwrap();
        assert_eq!(
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateSignatureRows),
            before
        );
    });
}
