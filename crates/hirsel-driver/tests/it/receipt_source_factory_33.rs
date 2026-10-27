//! Actual original-Lower/V observations; no receipt-aware native activation.
use hirsel_driver::{compiler_adapters, native::receipt_source};
use hirsel_engine::*;
use hirsel_rc::instance::{Actuals, key_of};
use hirsel_rc::sealed_source::SourceError;
use hirsel_types::{GenericArgument, Primitive, Type, TypeKind, TypeMemo, Uniqueness};
use std::sync::Arc;

const SOURCE: &str = "fn id[T](take value:T)->T {take value}\npub fn main()->u64 {7u64}";

fn engine(verify: Verify) -> (NativeOriginalReader, hirsel_syntax::DefPath) {
    let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    engine.verify = verify;
    let file = engine
        .sources
        .add("receipt-source-factory.hir", SOURCE.into())
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    let mut id = entry_path(&default_context());
    id.0.name = "id".into();
    let facts = crate::native_front_fixture::products(&mut engine, &[id.clone()]).unwrap();
    let reader = facts
        .original_context
        .reader(
            facts.originals.iter().map(|product| (**product).clone()),
            store,
        )
        .unwrap();
    (reader, id)
}

#[test]
fn actual_v_observation_preserves_original_modes_and_releases_the_body() {
    with_compiler_stack(|| {
        let (reader, path) = engine(Verify::On);
        let signature = reader
            .signatures()
            .signature(hirsel_syntax::DefId::intern(&path))
            .unwrap()
            .unwrap();
        let original = reader.read(&path).unwrap();
        let output = original.output().expect("actual V output");
        let source = output.observation_identity();
        let revision = original.implementation_revision();
        let types = reader.signatures().types().clone();
        let definition = original.definition();
        let body = definition.entry;
        let scope = definition.bodies[body.0 as usize].parameter_scope;
        let weak = Arc::downgrade(definition);
        let mut accounts = Vec::new();
        let mut keys = Vec::new();
        for uniqueness in [Uniqueness::Aliased, Uniqueness::Unique] {
            let mut ty = Type::with_kind(TypeKind::Primitive(Primitive::Bytes));
            ty.modes.uniqueness = uniqueness;
            let actuals = Actuals {
                binder: Some(scope),
                arguments: vec![GenericArgument::Type(types.intern(ty))],
                outer: vec![],
            };
            let base = key_of(&types, &path, body, &actuals, None, None).unwrap();
            let observed = receipt_source::named(
                &original,
                &base,
                &types,
                &signature,
                &actuals,
                &TypeMemo::new(),
            )
            .unwrap();
            assert_eq!(observed.entry().source_identity(), source);
            assert_eq!(observed.demand().source_identity(), source);
            assert_eq!(observed.entry().implementation_revision(), revision);
            assert_eq!(observed.demand().implementation_revision(), revision);
            assert_eq!(observed.entry().reference(), observed.demand().reference());
            assert_eq!(observed.entry().parameters().len(), 1);
            assert_eq!(
                &observed.entry().parameters()[0].ty,
                observed.entry().result()
            );
            keys.push(base);
            accounts.push(observed);
        }
        assert_eq!(keys[0], keys[1], "native key erases source uniqueness");
        assert_ne!(
            accounts[0].entry().result().source_type,
            accounts[1].entry().result().source_type
        );
        drop(original);
        drop(reader);
        assert!(
            weak.upgrade().is_none(),
            "compact observations retain no source body"
        );
        assert_eq!(accounts[0].entry().source_identity(), source);
        assert_eq!(accounts[1].demand().projection().parameters().len(), 1);
    });
}

#[test]
fn a_wrong_declaration_or_derived_native_key_cannot_select_original_source() {
    with_compiler_stack(|| {
        let (reader, path) = engine(Verify::On);
        let signature = reader
            .signatures()
            .signature(hirsel_syntax::DefId::intern(&path))
            .unwrap()
            .unwrap();
        let original = reader.read(&path).unwrap();
        let types = reader.signatures().types();
        let body = original.definition().entry;
        let actuals = Actuals {
            binder: Some(original.definition().bodies[body.0 as usize].parameter_scope),
            arguments: vec![GenericArgument::Type(
                types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes))),
            )],
            outer: vec![],
        };
        let base = key_of(types, &path, body, &actuals, None, None).unwrap();
        assert!(
            receipt_source::named(
                &original,
                &base,
                types,
                &signature,
                &actuals,
                &TypeMemo::new()
            )
            .is_ok()
        );
        let mut wrong = signature.as_ref().clone();
        wrong.fingerprint = Default::default();
        assert_eq!(
            receipt_source::named(&original, &base, types, &wrong, &actuals, &TypeMemo::new())
                .err(),
            Some(SourceError::Malformed(
                "original entry declaration identity"
            ))
        );
        let derived = base.derived("hirsel.test.receipt-derived.v1", &[b"not an original base"]);
        assert_eq!(
            receipt_source::named(
                &original,
                &derived,
                types,
                &signature,
                &actuals,
                &TypeMemo::new()
            )
            .err(),
            Some(SourceError::Malformed(
                "original named specialization key differs"
            ))
        );
    });
}

#[test]
fn unavailable_v_output_is_not_an_expected_empty_source_observation() {
    with_compiler_stack(|| {
        let (reader, path) = engine(Verify::Off);
        let signature = reader
            .signatures()
            .signature(hirsel_syntax::DefId::intern(&path))
            .unwrap()
            .unwrap();
        let original = reader.read(&path).unwrap();
        assert!(original.output().is_none());
        let types = reader.signatures().types();
        let body = original.definition().entry;
        let actuals = Actuals {
            binder: Some(original.definition().bodies[body.0 as usize].parameter_scope),
            arguments: vec![GenericArgument::Type(
                types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes))),
            )],
            outer: vec![],
        };
        let base = key_of(types, &path, body, &actuals, None, None).unwrap();
        assert_eq!(
            receipt_source::named(
                &original,
                &base,
                types,
                &signature,
                &actuals,
                &TypeMemo::new()
            )
            .err(),
            Some(SourceError::Unsupported(
                "receipt source observation needs the current original V output"
            ))
        );
    });
}

#[test]
fn sealed_callback_capture_129_original_graph_declares_owning_complete_profiles() {
    use hirsel_driver::load_program;
    with_compiler_stack(|| {
        use hirsel_driver::native::receipt_source::{
            factory::{NamedInput, named_graph},
            provider::Front,
        };
        use hirsel_lir::sealed_payload_source::{ReceiptMode, ReceiptPort};
        for name in [
            "MEM-4-sealed-callback-failure-run.hir",
            "MEM-4-sealed-callback-drop-cause-run.hir",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/MEM")
                .join(name);
            let mut program = load_program(&path).unwrap();
            program.check_for_execution().unwrap();
            let input = program.engine.export_native_input().unwrap();
            let validated = program.engine.import_native_input(input).unwrap();
            let input = validated.snapshot();
            let original = &input.definitions[&program.entry];
            let base = hirsel_layout::instance::InstanceKey::new(
                &program.entry,
                original.entry.0,
                &[],
                &input.types,
                None,
                None,
            )
            .unwrap();
            let provider = Front::new(validated.original_reader().unwrap());
            let roots = [NamedInput {
                base,
                actuals: hirsel_rc::instance::Actuals::none(),
            }];
            let mut cases = |_: &hirsel_rc::sealed_source::SourceCallableEntry, _, _| {
                panic!("this program has no unresolved held callable boundary")
            };
            let graph = named_graph(&provider, roots.clone(), &mut cases).unwrap();
            graph.recheck(&provider, roots, &mut cases).unwrap();
            let mut owned = 0;
            for request in &graph.requests {
                let Some(source) = request.receipt_source().unwrap() else {
                    continue;
                };
                source.check_structure().unwrap();
                let original = request.declaration().entry();
                for capture in &source.contract.captures {
                    let usage = original
                        .captures()
                        .iter()
                        .find(|c| c.ordinal == capture.ordinal)
                        .unwrap()
                        .usage;
                    let mode = match usage {
                        hirsel_types::UseKind::Borrow => ReceiptMode::Borrow,
                        hirsel_types::UseKind::Copy | hirsel_types::UseKind::Take => {
                            ReceiptMode::Take
                        }
                        hirsel_types::UseKind::Mut => panic!("mutable capture admitted"),
                    };
                    assert_eq!(capture.mode, mode);
                    let profile = source
                        .profile(ReceiptPort::Capture(capture.ordinal))
                        .unwrap();
                    assert_eq!(capture.ty.profile, profile.content().unwrap());
                    owned += usize::from(mode == ReceiptMode::Take);
                }
            }
            assert!(owned > 0, "{name} has no demanded owning capture");
        }
    });
}
