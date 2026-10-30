use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_lir::ir::{Symbol, SymbolKind};
use std::sync::Arc;

const SOURCE: &str = r#"
fn ordinary(x:u64)->u64 { 10u64/x }
pub fn main()->Result[(),()] {
 let mut s:Set[String]=set.empty()
 let insert:fn(mut Set[String],take String)->()=set.insert
 let _=insert(mut s,"key")
 let f=ordinary
 let _=f(2u64)
 Ok(())
}"#;

#[test]
fn v_owned_invocations_match_original_source_and_survive_a_fresh_body_free_hit() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().to_path_buf()));
        let session = |lower: bool| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("source-calls.hir", SOURCE.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if !lower {
                engine.adapters.lowering = None;
            }
            engine
        };
        let mut engine = session(true);
        let main = entry_path(&default_context());
        let front = crate::native_front_fixture::products(&mut engine, &[]).unwrap();
        let reader = front
            .original_context
            .reader(front.originals.iter().map(|product| (**product).clone()))
            .unwrap();
        let declarations =
            native::queries::declarations::NativeDeclarations::from_front(front.clone()).unwrap();
        let program = &declarations.program;
        let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
        signatures.extend(
            program
                .signatures
                .iter()
                .map(|(path, signature)| (path, (**signature).clone())),
        );
        let body = reader.read(&main).unwrap().definition().clone();
        let weak = Arc::downgrade(&body);
        let output = reader.input(&main).unwrap().0.unwrap().clone();
        let facts = output.invocations();
        assert_eq!(facts.owner().path(), &main);
        assert_eq!(facts.source_identity(), output.observation_identity());
        assert_eq!(facts.body_count() as usize, body.bodies.len());
        assert_eq!(facts.entry_body(), body.entry.0);
        let selected = Symbol {
            kind: SymbolKind::Function,
            definition: main.clone(),
            instance: Some(hirsel_syntax::ContentHash::of_bytes(
                b"independent selected instance",
            )),
        };
        let expected = hirsel_rc::invocations::source_instance(
            &program.types,
            &body,
            &signatures,
            &program.operations,
            &selected,
            body.entry,
        )
        .unwrap();
        let body_id = body.entry;
        let expected_entry = body.entry.0;
        let compact =
            |facts: &hirsel_validate::VerifiedInvocations, source, selected: &Symbol, body| {
                hirsel_rc::invocations::source_instance_from_output(
                    facts,
                    source,
                    &program.operations,
                    selected,
                    body,
                )
            };
        assert_eq!(
            compact(&facts, &main, &selected, body_id).unwrap(),
            expected
        );
        assert!(
            expected
                .invocations
                .iter()
                .any(|call| call.primitive.is_some())
        );
        assert!(
            expected
                .invocations
                .iter()
                .any(|call| call.callee.is_none())
        );
        assert!(
            compact(
                &facts,
                &main,
                &selected,
                hirsel_tir::BodyId(facts.body_count())
            )
            .is_err()
        );
        let wrong_source = hirsel_syntax::DefPath::core("wrong");
        assert!(compact(&facts, &wrong_source, &selected, body_id).is_err());
        let mut unbound = selected.clone();
        unbound.instance = None;
        assert!(compact(&facts, &main, &unbound, body_id).is_err());
        let operations = program.operations.clone();
        drop(body);
        drop(signatures);
        drop(declarations);
        drop(reader);
        drop(front);
        drop(engine);
        crate::native_front_fixture::release_host();
        assert!(
            weak.upgrade().is_none(),
            "facts retain no original Definition"
        );
        let mut fresh = session(false);
        let front = crate::native_front_fixture::products(&mut fresh, &[]).unwrap();
        let reader = front
            .original_context
            .reader(front.originals.iter().map(|product| (**product).clone()))
            .unwrap();
        let reopened = reader.input(&main).unwrap().0.unwrap().clone();
        assert_eq!(reopened.invocations().entry_body(), expected_entry);
        assert_eq!(fresh.recompute_count(&main, QueryKind::Lower), 0);
        assert_eq!(
            reopened.observation_identity(),
            output.observation_identity()
        );
        drop(fresh);
        assert_eq!(
            hirsel_rc::invocations::source_instance_from_output(
                &reopened.invocations(),
                &main,
                &operations,
                &selected,
                body_id
            )
            .unwrap(),
            expected
        );
    });
}
