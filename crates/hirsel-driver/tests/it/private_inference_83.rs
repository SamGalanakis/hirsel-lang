use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::MemStore, *};
use std::sync::Arc;

#[test]
fn admitted_private_facts_reuse_unchanged_inputs_and_refuse_body_and_membership_edits() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"private_facts","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let store = Arc::new(MemStore::default());
        let budget = ProcessBudget::new(512 << 20);
        let source = "fn value(data: Bytes)->u8 { data[0] }\npub fn main()->u8 { 1u8 }\n";
        let check = |text: &str, expected: usize| {
            std::fs::write(&path, text).unwrap();
            let mut program = COMPILER
                .with_config(CompileConfig {
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let mut value = program.entry.clone();
            value.0.name = "value".into();
            let reservation = budget.reserve(256 << 20).unwrap();
            let reservation = program
                .engine
                .prepare_private_inference(reservation)
                .unwrap();
            assert_eq!(reservation.bytes(), 256 << 20);
            assert_eq!(
                program
                    .engine
                    .recompute_count(&value, QueryKind::PrivateInference),
                expected
            );
            for kind in [QueryKind::Resolve, QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    program.engine.recompute_count(&value, kind),
                    0,
                    "inference enters S5: {kind:?}"
                );
            }
            hirsel_types::SignatureProvider::signature(
                &mut program.engine,
                hirsel_syntax::DefId::intern(&value),
            )
            .unwrap();
            let reads = program
                .engine
                .memo_reads(&value, QueryKind::Signature)
                .unwrap();
            assert!(
                reads
                    .iter()
                    .any(|(key, _)| matches!(key, memo::ReadKey::NameSet(..)))
            );
            assert!(reads.iter().any(
                |(key, _)| matches!(key, memo::ReadKey::CitedText(id) if id.path() == &value)
            ));
            drop(reservation);
            assert_eq!(budget.reserved(), 0);
        };
        check(source, 1);
        check(source, 0);
        check(&source.replace("{ 1u8 }", "{ 2u8 }"), 0);
        check(&source.replace("data[0]", "data[1]"), 1);
        check(&(source.to_owned() + "fn sibling()->u8 { 3u8 }\n"), 1);
    });
}

#[test]
fn private_facts_without_persistence_reuse_reads_after_an_unrelated_body_edit() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let mut value = entry_path(&context);
        value.0.name = "value".into();
        let budget = ProcessBudget::new(512 << 20);
        for (source, count) in [
            (
                "fn value(data: Bytes)->u8 { data[0] }\npub fn main()->u8 { 1u8 }\n",
                1,
            ),
            (
                "fn value(data: Bytes)->u8 { data[0] }\npub fn main()->u8 { 2u8 }\n",
                1,
            ),
            (
                "fn value(data: Bytes)->u8 { data[1] }\npub fn main()->u8 { 2u8 }\n",
                2,
            ),
        ] {
            let file = engine.sources.add("local.hir", source.into()).unwrap();
            engine.parse_file(file, &context).unwrap();
            let reservation = engine
                .prepare_private_inference(budget.reserve(256 << 20).unwrap())
                .unwrap();
            assert_eq!(
                engine.recompute_count(&value, QueryKind::PrivateInference),
                count
            );
            assert_eq!(engine.recompute_count(&value, QueryKind::Check), 0);
            drop(reservation);
            assert_eq!(budget.reserved(), 0);
        }
    });
}
