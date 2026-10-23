use hirsel_driver::*;
use hirsel_engine::store::{Action, MemStore, ResultStore};
use hirsel_engine::*;
use hirsel_rc::instance::Actuals;
use hirsel_syntax::ContentHash;
use native::queries::ready::{EMISSION_CONTEXT_DOMAIN, ReadyMiss, ReadyQueries, ReadySourceFacts};
use std::sync::Arc;

#[test]
fn ready_query_keeps_c_authority_after_releasing_source_bodies() {
    with_compiler_stack(|| {
        let transport = Arc::new(MemStore::default());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(transport.clone());
        let file = engine
            .sources
            .add("ready-query.hir", "pub fn main()->u64{1u64}".into())
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), transport.clone());
        let mut declarations = native::queries::declarations::NativeDeclarations::collect(
            &mut engine,
            &mut discovery,
            &[],
        )
        .unwrap();
        assert!(declarations.program.definitions.is_empty());
        let main = entry_path(&default_context());
        let body = declarations.body(&mut engine, &main).unwrap();
        let source = &declarations.sources[&main];
        let source_observation = source
            .lower
            .as_ref()
            .expect("actual source V output")
            .observation_identity();
        let original = hirsel_rc::sealed_source::SourceInput::capture(
            source_observation,
            source.source.implementation_revision,
            body.clone(),
        );
        let weak = Arc::downgrade(&body);
        let program = declarations.program;
        let context = declarations.context;
        drop(engine);
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let mut layouts = hirsel_layout::layout::LayoutBuilder::new(&target, &program.owners);
        let (instances, _) =
            hirsel_rc::instance::seed_instances(&program, &context, &mut layouts).unwrap();
        let id = instances.find(&program, &main, &Actuals::none()).unwrap();
        let expected_source = original
            .derive(&instances.view(id).key, &program.types, &[], Ok)
            .unwrap();
        assert_eq!(expected_source.witness().source, source_observation);
        assert!(expected_source.events().is_empty());
        assert!(expected_source.payloads().is_empty());
        drop(original);
        let (mut instances, _) = hirsel_rc::instance::discover_instance(
            &program,
            &context,
            &mut layouts,
            instances,
            id,
            body,
        )
        .unwrap();
        let imported =
            hirsel_rc::import::import_instance(&program, &mut instances, id, None, &mut layouts)
                .unwrap();
        for view in &mut instances.views {
            view.release_definition();
        }
        assert!(
            weak.upgrade().is_none(),
            "Ready uses compact source contracts and the accepted Value only"
        );
        let table = layouts.finish();
        let unavailable = program.operation_registry();
        let mut operations =
            hirsel_rc::sealed_source::SourceDeclarations::new(program.operation_registry());
        operations.insert(expected_source).unwrap();
        let manifest =
            hirsel_core_contract::Manifest::skeleton(program.operations.values().cloned());
        let inputs = hirsel_lir::validate::Inputs {
            target: &target,
            layouts: &table,
            manifest: &manifest,
            operations: &operations,
            discharge: None,
            memo: Default::default(),
        };
        let checked = hirsel_lir::validate::validate(imported.unit, &inputs).unwrap();
        let value = checked.into_raw();
        let key = native::queries::InstanceAddress {
            instance: instances.view(id).key.clone(),
            target_abi: target.digest(),
            policy: ContentHash::of_bytes(b"Ready component policy"),
        };
        let selected = key
            .instance
            .derived("selected Ready test", &[b"implementation"]);
        let nested = selected.derived("nested Ready test", &[b"implementation"]);
        assert!(key.instance.is_same_or_derived_from(&key.instance));
        assert!(nested.is_same_or_derived_from(&key.instance));
        assert!(nested.is_same_or_derived_from(&selected));
        assert!(!key.instance.is_same_or_derived_from(&selected));
        assert!(
            !selected.is_same_or_derived_from(&key.instance.derived("sibling Ready test", &[]))
        );
        let action = Action(ContentHash::of_bytes(b"Ready complete component action"));
        let emission = transport
            .put_content(EMISSION_CONTEXT_DOMAIN, b"{}")
            .unwrap();
        let mut options = native::Options::new(target.clone());
        options.validation = native::Validation::Ready;
        let finalizers = hirsel_rc::cleanup::Finalizers::planned(&instances, &table);
        let source_facts = ReadySourceFacts::for_instance(&instances, id);
        let source_bytes = source_facts.encode().unwrap();
        let source_facts = ReadySourceFacts::decode(&source_bytes).unwrap();
        assert_eq!(source_facts.encode().unwrap(), source_bytes);
        drop(instances);
        assert!(weak.upgrade().is_none());
        let environment = hirsel_lir::validate::NativeEnvironment {
            target: &target,
            manifest: &manifest,
            operations: &operations,
            discharge: None,
        };
        let miss = |value| ReadyMiss {
            value,
            options: &options,
            source: &source_facts,
            finalizers: &finalizers,
            inputs: &inputs,
            additional_abis: &[],
            statics: &[],
            emission_context: emission,
            debug: true,
            optimizer_reads: Vec::new(),
        };
        let mut queries = ReadyQueries::new(transport.clone());
        assert!(queries.lookup(&key, action, &environment).is_none());
        let (_, product) = queries
            .publish_from_value(&key, action, miss(value.clone()))
            .unwrap();
        assert_eq!(
            queries
                .lookup(&key, action, &environment)
                .unwrap()
                .product
                .record,
            product.product.record
        );
        let unavailable_environment = hirsel_lir::validate::NativeEnvironment {
            operations: &unavailable,
            ..environment
        };
        assert!(
            queries
                .lookup(&key, action, &unavailable_environment)
                .is_none(),
            "independently expected-empty original source cannot become unavailable"
        );
        assert!(queries.lookup(&key, action, &environment).is_some());
        let summary_bytes = transport.get_content(product.unit_summary).unwrap();
        let keys = hirsel_lir::LayoutKeys::new(&table);
        let summary = hirsel_lir::fragment::UnitSummary::decode(
            &summary_bytes,
            &|key| {
                keys.all()
                    .iter()
                    .position(|current| current == key)
                    .map(|i| hirsel_lir::LayoutId(i as u32))
            },
            &|key| {
                table
                    .fn_abis
                    .iter()
                    .position(|abi| keys.fn_abi(abi) == *key)
                    .map(|i| hirsel_lir::FnAbiId(i as u32))
            },
        )
        .unwrap();
        assert_eq!(summary.definition, main);
        assert_eq!(summary.instance, key.instance.digest());
        assert_eq!(queries.counts.transformations, 1);
        assert_eq!(queries.counts.validations, 1);
        let next = Action(ContentHash::of_bytes(b"Ready next action, equal output"));
        let (_, backdated) = queries
            .publish_from_value(&key, next, miss(value.clone()))
            .unwrap();
        assert_eq!(backdated.product.record, product.product.record);
        let mut header_only = value.clone();
        header_only.header.key =
            hirsel_lir::objects::UnitKey(ContentHash::of_bytes(b"changed nonsemantic Ready key"));
        let (_, stable) = queries
            .publish_from_value(&key, next, miss(header_only))
            .unwrap();
        assert_eq!(stable.product.record, backdated.product.record);
        let mut changed = value;
        let constant = changed
            .tables
            .constants
            .iter_mut()
            .find(|constant| constant.value == hirsel_lir::ir::ConstantValue::Integer(1))
            .expect("the source literal is 1u64");
        constant.value = hirsel_lir::ir::ConstantValue::Integer(2);
        let error = queries
            .publish_from_value(&key, next, miss(changed))
            .err()
            .expect("unequal output under unchanged action must refuse");
        assert!(error.contains("unchanged complete action"));
        assert_eq!(
            queries
                .lookup(&key, next, &environment)
                .unwrap()
                .product
                .record,
            backdated.product.record
        );
        let mut foreign = ReadyQueries::new(transport.clone());
        assert!(foreign.lookup(&key, next, &environment).is_none());
        let forged_emission = transport
            .put_content(EMISSION_CONTEXT_DOMAIN, b"forged")
            .unwrap();
        let bytes = serde_json::to_vec(&(
            true,
            product.ready,
            product.layout_closure,
            forged_emission,
            product.unit_summary,
            Vec::<(ContentHash, hirsel_engine::store::Reference)>::new(),
        ))
        .unwrap();
        let forged = transport
            .put_content(native::queries::READY_RECORD_DOMAIN, &bytes)
            .unwrap();
        transport.associate_product(product.product.address, action, forged);
        assert!(queries.lookup(&key, action, &environment).is_none());
        assert_eq!(queries.counts.transformations, 4);
        assert!(weak.upgrade().is_none());
    });
}
