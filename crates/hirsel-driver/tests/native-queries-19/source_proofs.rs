use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_proof_export::sidecar::{Budget, Checker, Record, Verdict};
use hirsel_syntax::ContentHash;
use native::queries::{
    declarations::NativeDeclarations, demand::DemandInstances, source_inputs::NativeInstanceInputs,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const SOURCE: &str = "fn bump(x:u64)->u64{x+1u64}\npub fn main()->u64{bump(1u64)}";
fn evidence(engine: &mut QueryEngine) -> native::Evidence {
    let input = engine
        .proof_export_input(&entry_path(&default_context()))
        .unwrap();
    let export = hirsel_proof_export::build(&input);
    let obligation = export
        .definitions
        .iter()
        .find(|section| section.path.0.name == "bump")
        .unwrap()
        .obligations
        .iter()
        .find(|obligation| obligation.id.rule.as_str() == "SEM-1")
        .unwrap();
    let checker = Checker::parse("source-control@1").unwrap();
    native::Evidence {
        records: vec![Record {
            revision: export.header.program_revision.unwrap(),
            obligation: obligation.id.to_string(),
            checksum: obligation.checksum,
            checker: checker.clone(),
            budget: Budget {
                unit: "steps".into(),
                count: 1000,
            },
            verdict: Verdict::Proved,
        }],
        trusted: vec![checker],
    }
}
fn session(store: Arc<store::DirStore>) -> (QueryEngine, hirsel_span::FileId) {
    let mut engine =
        QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store);
    let file = engine
        .sources
        .add("source-proof.hir", SOURCE.into())
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    (engine, file)
}
fn source(engine: &mut QueryEngine, store: Arc<store::DirStore>) -> Arc<NativeInstanceInputs> {
    let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
    let declarations = NativeDeclarations::collect(engine, &mut discovery, &[]).unwrap();
    NativeInstanceInputs::complete(engine, declarations, store).unwrap()
}
fn products(
    source: &Arc<NativeInstanceInputs>,
    store: Arc<store::DirStore>,
    options: &native::Options,
) -> (BTreeMap<String, native::queries::InstanceProduct>, u64) {
    let mut provider = source.provider();
    let mut graph = DemandInstances::new_from_source(
        &provider,
        store,
        options.target.clone(),
        ContentHash::of_bytes(b"source proof native control"),
    );
    let mut pending = graph.roots_from(&provider).unwrap();
    let mut seen = BTreeSet::new();
    let mut results = BTreeMap::new();
    while let Some(key) = pending.pop() {
        if !seen.insert(key.instance.encoding().to_vec()) {
            continue;
        }
        let output = graph.instance_from(&mut provider, &key, options).unwrap();
        pending.extend(output.dependencies.iter().cloned());
        results.insert(
            key.instance.definition().0.name.to_string(),
            output.product.as_ref().clone(),
        );
    }
    assert_eq!(graph.counts().validations, graph.imports);
    (results, graph.imports)
}
#[test]
fn source_proof_assessment_keeps_exported_tir_identity_and_native_discharge_validation() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().to_path_buf()));
        let (mut engine, _) = session(store.clone());
        let current = evidence(&mut engine);
        let selected = source(&mut engine, store.clone());
        let main = entry_path(&default_context());
        let original = engine.lower_to_tir(&main).unwrap().value;
        let weak = Arc::downgrade(&original);
        drop(original);
        let proofs =
            native::Proofs::assess_source(&mut engine, &main, &selected, &current).unwrap();
        let mut stale = current.clone();
        stale.records[0].checksum.0[0] ^= 1;
        let refused = native::Proofs::assess_source(&mut engine, &main, &selected, &stale).unwrap();
        assert!(refused.discharge.0.is_empty());
        assert!(
            refused
                .report_discharge(&refused.discharge)
                .iter()
                .any(|line| line.contains("kept"))
        );
        assert!(!proofs.discharge.0.is_empty());
        drop(engine);
        assert!(weak.upgrade().is_none());
        let mut options = native::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
        let (baseline, imports) = products(&selected, store.clone(), &options);
        assert!(imports >= 2);
        options.discharge = proofs.discharge.clone();
        let (proved, imports) = products(&selected, store.clone(), &options);
        assert_eq!(imports, 1);
        assert_ne!(proved["bump"].value, baseline["bump"].value);
        assert_eq!(
            proved["main"].product, baseline["main"].product,
            "unaffected caller has no proof/body dependency"
        );
        assert!(
            proofs
                .report_discharge(&options.discharge)
                .iter()
                .any(|line| line.contains(": removed"))
        );
        let (fresh, imports) = products(&selected, store, &options);
        assert_eq!(imports, 0);
        assert_eq!(fresh["bump"].product, proved["bump"].product);
    });
}
#[test]
fn source_proof_assessment_refuses_a_current_proof_against_a_different_selected_original() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().to_path_buf()));
        let (mut engine, file) = session(store.clone());
        let old = source(&mut engine, store.clone());
        let main = entry_path(&default_context());
        let stale = evidence(&mut engine);
        let file = engine
            .sources
            .add_snapshot(file, SOURCE.replace("x+1u64", "x+2u64"))
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let current = evidence(&mut engine);
        let error = native::Proofs::assess_source(&mut engine, &main, &old, &current)
            .err()
            .unwrap();
        assert!(error.message.contains("selected original TIR"), "{error:?}");
        let current_inputs = source(&mut engine, store);
        let stale =
            native::Proofs::assess_source(&mut engine, &main, &current_inputs, &stale).unwrap();
        assert!(stale.discharge.0.is_empty());
        assert!(
            !native::Proofs::assess_source(&mut engine, &main, &current_inputs, &current)
                .unwrap()
                .discharge
                .0
                .is_empty()
        );
    });
}
