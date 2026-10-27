use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn checked_signature_inputs_keep_exact_coverage_private_views_and_source_owner() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), "pub opaque type Hidden: () uses {} { value:u64 }\nfn read(h:Hidden)->u64{h.value}\npub fn main()->u8{1u8}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"headers","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let mut program = COMPILER
            .with_config(CompileConfig {
                store: Arc::new(MemStore::default()),
                ..Default::default()
            })
            .with_owned_engines()
            .load_program(&manifest)
            .unwrap();
        let definitions = program.engine.front_source_roots();
        let budget = ProcessBudget::new(32 << 20);
        let weight = MemoryWeight {
            retained: 64 << 10,
            scratch: 8 << 20,
            stack: 0,
        };
        let front = program
            .engine
            .front_body_queries(
                &definitions,
                weight,
                budget.reserve(8 << 20).unwrap(),
                crate::scheduled_front::errors(),
            )
            .unwrap();
        let before = budget.reserved();
        assert!(
            program
                .engine
                .front_checked_signatures(front.clone(), budget.reserve(0).unwrap())
                .is_err()
        );
        assert_eq!(budget.reserved(), before);
        let required = program
            .engine
            .front_checked_signatures_bytes(&front)
            .unwrap();
        assert!(
            program
                .engine
                .front_checked_signatures(front.clone(), budget.reserve(required - 1).unwrap())
                .is_err()
        );
        let inputs = program
            .engine
            .front_checked_signatures(front.clone(), budget.reserve(required).unwrap())
            .unwrap();
        assert!(inputs.matches(&front));
        assert_eq!(inputs.environment(), &program.engine.proof_environment());
        for definition in &definitions {
            let section = inputs.section(definition).unwrap();
            let expected = program.engine.item_signature(definition).unwrap();
            assert_eq!(section.public, *expected.value);
            assert_eq!(
                section.dependencies,
                program.engine.signature_coverage(definition).unwrap()
            );
            assert_eq!(front.recompute_count(definition, QueryKind::Check), 0);
            assert_eq!(program.engine.lower_adapter_calls(definition), 0);
        }
        let mut hidden = program.entry.clone();
        hidden.0.name = "Hidden".into();
        assert!(inputs.owner_view(&hidden).is_some());
        let owner = inputs.owner_view(&hidden).unwrap();
        let public = inputs
            .signatures()
            .find(|(path, _)| **path == hidden)
            .unwrap()
            .1;
        assert_ne!(owner, public);
        assert!(
            matches!(&public.kind, hirsel_types::SignatureKind::Type(t) if t.representation.is_none())
        );
        assert!(
            matches!(&owner.kind, hirsel_types::SignatureKind::Type(t) if t.representation.is_some())
        );
        assert_eq!(front.live_body_count(), 0);
        let second = program
            .engine
            .front_body_queries(
                &definitions,
                weight,
                budget.reserve(8 << 20).unwrap(),
                crate::scheduled_front::errors(),
            )
            .unwrap();
        assert!(
            !inputs.matches(&second),
            "the same signature snapshot alone is not a source selection"
        );
        drop(second);
        let expected = program.engine.proof_export_input(&program.entry).unwrap();
        for definition in &definitions {
            assert_eq!(
                inputs.section(definition).as_ref(),
                expected.signatures.get(definition)
            );
        }
        drop(expected);
        drop(program);
        drop(front);
        assert_eq!(budget.reserved(), before + required);
        assert!(
            inputs.section(&hidden).is_some(),
            "header inspection keeps its source owner alive"
        );
        drop(inputs);
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
fn generated_ps9_headers_require_the_admitted_registration_view() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"),
            "fn make()->Result[Body[fn()->u8 uses {}],String]{Ok(codegen.body(\"fn()->u8 uses {} {7u8}\"))}\npub fn main()->u8 uses {} = comptime make()\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"generated_headers","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let config = CompileConfig {
            store: Arc::new(MemStore::default()),
            hold_bytes: 0,
            ..Default::default()
        };
        let mut session = session::Session::open_configured(&manifest, config);
        let output = session
            .request(hirsel_driver::requests::CompileOperation::ExportTir)
            .unwrap();
        let hirsel_driver::requests::CompileResult::ExportTir(text) = &output.result else {
            panic!("wrong export operation")
        };
        assert!(text.contains("generated_headers@1/main"), "{text}");
        assert!(text.contains("generated_headers@1/make"), "{text}");
    });
}
