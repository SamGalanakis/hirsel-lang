//! Durable completion consumes compact facts without loading the Modes body.
use hirsel_driver::*;
use hirsel_engine::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Observed {
    inner: store::MemStore,
    reads: AtomicUsize,
    writes: AtomicUsize,
    content_reads: AtomicUsize,
    content_writes: AtomicUsize,
    mode_reads: AtomicUsize,
    summary_reads: AtomicUsize,
    summary_content_reads: AtomicUsize,
    indexes: Mutex<std::collections::BTreeSet<hirsel_syntax::ContentHash>>,
    forbid_content_reads: AtomicBool,
}
impl store::ResultStore for Observed {
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if key.2 == QueryKind::Modes { self.mode_reads.fetch_add(1, Ordering::Relaxed); }
        if key.2 == QueryKind::Summary { self.summary_reads.fetch_add(1, Ordering::Relaxed); }
        store::ResultStore::get(&self.inner, key)
    }
    fn put(&self, key: &store::EntryKey, bytes: &[u8]) {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.indexes.lock().unwrap().insert(key.0);
        store::ResultStore::put(&self.inner, key, bytes);
    }
    fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
        assert!(
            !self.forbid_content_reads.load(Ordering::Relaxed)
                || ![QueryKind::Modes, QueryKind::Summary, QueryKind::Lower].iter().any(|kind| reference.domain.kind == *kind as u32 + 16),
            "stale dependency action materialized a payload"
        );
        self.content_reads.fetch_add(1, Ordering::Relaxed);
        if reference.domain.kind == QueryKind::Summary as u32 + 16 { self.summary_content_reads.fetch_add(1, Ordering::Relaxed); }
        store::ResultStore::get_content(&self.inner, reference)
    }
    fn put_content(&self, domain: store::Domain, bytes: &[u8]) -> Option<store::Reference> {
        self.content_writes.fetch_add(1, Ordering::Relaxed);
        store::ResultStore::put_content(&self.inner, domain, bytes)
    }
    fn name(&self) -> &'static str {
        "observed-completion"
    }
}

fn publish(store: Arc<Observed>, source: &str, jobs: usize) -> (QueryEngine, Vec<PublishedResult>) {
    let mut engine =
        QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store);
    let file = engine.sources.add("completion.hir", source.into()).unwrap();
    let parsed = engine.parse_file(file, &default_context()).unwrap();
    assert!(parsed.diagnostics.is_empty());
    let definitions = parsed
        .value
        .items
        .iter()
        .map(|s| s.item.definition.clone())
        .collect();
    let outputs = engine.publish_definitions(
        definitions,
        hirsel_intern::jobs::Jobs::new(jobs),
        memo::Retention::Batch,
    );
    (engine, outputs)
}

#[test]
fn compact_store_hits_do_not_lookup_or_materialize_modes_bodies() {
    with_compiler_stack(|| {
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(Observed::default());
            let source = "fn value(x:u32)->u32{x+1u32}\nfn caller()->u32{value(4u32)}\n";
            let (_, seeded) = publish(store.clone(), source, jobs);
            assert!(seeded.iter().all(|o| o.result.is_ok()));
            store.reads.store(0, Ordering::Relaxed);
            store.mode_reads.store(0, Ordering::Relaxed);
            store.summary_reads.store(0, Ordering::Relaxed);
            store.summary_content_reads.store(0, Ordering::Relaxed);
            store.writes.store(0, Ordering::Relaxed);
            store.content_reads.store(0, Ordering::Relaxed);
            store.content_writes.store(0, Ordering::Relaxed);
            let (mut fresh, actual) = publish(store.clone(), source, jobs);
            assert!(actual.iter().all(|o| o.result.is_ok()));
            assert_eq!(
                store.summary_reads.load(Ordering::Relaxed),
                2,
                "one compact record per definition; no Modes lookup"
            );
            assert_eq!(store.writes.load(Ordering::Relaxed), 0);
            assert_eq!(store.content_writes.load(Ordering::Relaxed), 0);
            assert_eq!(store.summary_content_reads.load(Ordering::Relaxed), 2);
            assert_eq!(store.mode_reads.load(Ordering::Relaxed), 0);
            assert_eq!(fresh.store_adoptions(), 4, "two signatures and two completions");
            for output in &actual {
                for phase in [QueryKind::Check, QueryKind::Modes] {
                    assert_eq!(fresh.recompute_count(&output.definition, phase), 0);
                }
                let before = fresh.memo_stats().recomputed.clone();
                fresh
                    .checked_definition_summary(hirsel_syntax::DefId::intern(&output.definition))
                    .unwrap();
                assert_eq!(fresh.memo_stats().recomputed, before);
            }
            let changed = source
                .replace("x:u32", "x:u64")
                .replace("->u32{x+1u32}", "->u64{x+1u64}");
            store.forbid_content_reads.store(true, Ordering::Relaxed);
            let (invalidated_engine, invalidated) = publish(store.clone(), &changed, jobs);
            store.forbid_content_reads.store(false, Ordering::Relaxed);
            let (cold_engine, cold) = publish(Arc::new(Observed::default()), &changed, jobs);
            let render = |engine: &QueryEngine, outcomes: &[PublishedResult]| {
                outcomes
                    .iter()
                    .map(|o| {
                        let diagnostics = match &o.result {
                            Ok(p) => &p.diagnostics,
                            Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
                            Err(error) => panic!("{error:?}"),
                        };
                        diagnostics
                            .iter()
                            .map(|d| d.render_human(&engine.sources))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                render(&invalidated_engine, &invalidated),
                render(&cold_engine, &cold)
            );
            assert!(invalidated.iter().any(|o| o.result.is_err()));
        }
    });
}

#[test]
fn settled_summary_identity_ignores_source_and_positions_but_observes_calls() {
    with_compiler_stack(|| {
        let source = "fn value(x:u32)->u32{x+1u32}\nfn other(x:u32)->u32{x}\nfn caller()->u32{value(4u32)}\n";
        let store = Arc::new(Observed::default());
        let (mut resident, original) = publish(store.clone(), source, 1);
        assert!(original.iter().all(|o| o.result.is_ok()));
        let caller = original
            .iter()
            .find(|o| o.definition.0.name == "caller")
            .unwrap();
        let id = hirsel_syntax::DefId::intern(&caller.definition);
        let original_key = resident.definition_summary_identity(id).unwrap().unwrap();
        for jobs in [1, 2, 4, 8] {
            let shifted = source
                .replace("x+1u32", "x+2u32")
                .replace("{value(4u32)}", "{  value(4u32) }");
            let file = resident
                .sources
                .add("completion.hir", shifted.clone())
                .unwrap();
            resident.parse_file(file, &default_context()).unwrap();
            assert_eq!(
                resident.definition_summary_identity(id).unwrap(),
                Some(original_key)
            );
            let (mut fresh, outputs) = publish(store.clone(), &shifted, jobs);
            assert!(outputs.iter().all(|o| o.result.is_ok()));
            assert_eq!(
                fresh.definition_summary_identity(id).unwrap(),
                Some(original_key)
            );
            let changed = shifted.replace("value(4u32)", "other(4u32)");
            let (mut fresh, outputs) = publish(store.clone(), &changed, jobs);
            assert!(outputs.iter().all(|o| o.result.is_ok()));
            assert_ne!(
                fresh.definition_summary_identity(id).unwrap(),
                Some(original_key)
            );
        }
    });
}

#[test]
fn source_growth_refreshes_pending_roots_and_reuses_unrelated_components() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = "fn first[T:copyable](x:T){second((copy x,copy x))}\nfn second[U:copyable](x:U){first(x)}\nfn separate[T](x:T){separate(x)}\nfn ordinary()->u32{1u32}\n";
        let parse = |engine: &mut QueryEngine, source: &str| {
            let file = engine.sources.add("growth.hir", source.into()).unwrap();
            let output = engine.parse_file(file, &default_context()).unwrap();
            assert!(output.diagnostics.is_empty());
        };
        parse(&mut engine, source);
        let original = engine.generic_graph_diagnostics().unwrap();
        assert_eq!(original.len(), 1);
        assert_eq!(original[0].rule.as_str(), "GEN-5");
        let before = engine.work_stats();
        assert_eq!(engine.generic_graph_diagnostics().unwrap(), original);
        assert_eq!(engine.work_stats().since(before).gen5_visits, 0);
        let body_only = source.replace("{1u32}", "{2u32}");
        parse(&mut engine, &body_only);
        let before = engine.work_stats();
        let unchanged = engine.generic_graph_diagnostics().unwrap();
        let work = engine.work_stats().since(before);
        assert_eq!(unchanged, original);
        assert_eq!(work.gen5_visits, 1, "{work:?}");
        assert_eq!(work.growth_components, 0);
        let finite = body_only.replace("second((copy x,copy x))", "second(x)");
        parse(&mut engine, &finite);
        let before = engine.work_stats();
        assert!(engine.generic_graph_diagnostics().unwrap().is_empty());
        let work = engine.work_stats().since(before);
        assert_eq!(work.gen5_visits, 1, "{work:?}");
        assert!(work.growth_components > 0, "{work:?}");
        assert!(
            work.growth_component_hits > 0,
            "separate SCC should survive: {work:?}"
        );
        // Splitting and merging the formerly recursive component must update
        // its membership before admitting a saved verdict.
        let split = source.replace("{first(x)}", "{let _=x}");
        parse(&mut engine, &split);
        assert!(engine.generic_graph_diagnostics().unwrap().is_empty());
        parse(&mut engine, source);
        let merged = engine.generic_graph_diagnostics().unwrap();
        assert_eq!(merged.len(), 1);
        assert!(
            engine
                .sources
                .slice(merged[0].primary)
                .unwrap()
                .contains("copy x")
        );
        // Position-only changes preserve the semantic result, with the new
        // position product supplying the actual diagnostic location.
        let moved = source.replace("{second", "{  second");
        parse(&mut engine, &moved);
        let before = engine.work_stats();
        let errors = engine.generic_graph_diagnostics().unwrap();
        assert_eq!(errors.len(), 1);
        assert_ne!(errors[0].primary, merged[0].primary);
        assert!(
            engine
                .sources
                .slice(errors[0].primary)
                .unwrap()
                .contains("copy x")
        );
        assert_eq!(engine.work_stats().since(before).growth_components, 0);
        // Removing a root retires its old edges and the stale growing cycle.
        let removed = moved
            .lines()
            .filter(|line| !line.starts_with("fn first"))
            .collect::<Vec<_>>()
            .join("\n");
        // Remove both mutually recursive declarations so the source remains valid.
        let removed = removed
            .lines()
            .filter(|line| !line.starts_with("fn second"))
            .collect::<Vec<_>>()
            .join("\n");
        parse(&mut engine, &removed);
        assert!(engine.generic_graph_diagnostics().unwrap().is_empty());
    });
}

#[test]
fn front_store_retains_prior_actions_at_one_logical_address() {
    with_compiler_stack(|| {
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(Observed::default());
            let source = "fn value()->u32{1u32}\nfn caller()->u32{value()}\n";
            let (_, outputs) = publish(store.clone(), source, jobs);
            assert!(outputs.iter().all(|o| o.result.is_ok()));
            let count = store.indexes.lock().unwrap().len();
            assert_eq!(count, 6, "Signature, Modes and Summary are separate domains");
            let changed = source.replace("{1u32}", "{2u32}");
            let (edited, outputs) = publish(store.clone(), &changed, jobs);
            assert!(outputs.iter().all(|o| o.result.is_ok()));
            assert_eq!(
                store.indexes.lock().unwrap().len(),
                count,
                "body edits do not allocate new addresses"
            );
            let value = outputs
                .iter()
                .find(|o| o.definition.0.name == "value")
                .unwrap();
            assert!(edited.recompute_count(&value.definition, QueryKind::Modes) > 0);
            store.writes.store(0, Ordering::Relaxed);
            let (reverted, outputs) = publish(store.clone(), source, jobs);
            assert!(outputs.iter().all(|o| o.result.is_ok()));
            assert_eq!(reverted.store_adoptions(), 4, "two signatures and two completions");
            assert_eq!(
                store.writes.load(Ordering::Relaxed),
                0,
                "original action remains available"
            );
        }
    });
}

fn signature_session(store: Arc<dyn store::ResultStore>, source: &str, normalize: bool) -> QueryEngine {
    let mut adapters = compiler_adapters();
    if !normalize { adapters.signature = None; }
    let mut engine = QueryEngine::new(default_environment(), adapters).with_result_store(store);
    if !normalize {
        engine.sources.add("earlier.hir", "fn unrelated(){}\n".into()).unwrap();
    }
    let file = engine.sources.add("signatures.hir", source.into()).unwrap();
    assert!(engine.parse_file(file, &default_context()).unwrap().diagnostics.is_empty());
    engine
}
fn definition(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}
#[test]
fn signature_store_reuses_headers_and_preserves_owned_views_and_resolver_syntax() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source = "pub opaque type Hidden: plain uses {} {value:u32}\nfn value(x:u32)->u32{x+1u32}\nfn caller(x:Hidden)->Hidden{x}\n";
        let mut seed = signature_session(store.clone(), source, true);
        let names = ["Hidden", "value", "caller"];
        let fingerprints: Vec<_> = names.iter().map(|name| seed.item_signature(&definition(name)).unwrap().value.fingerprint).collect();
        let owner = definition("Hidden");
        let shape = seed.owner_representation(&owner, &owner).unwrap().representation_revision;
        drop(seed);
        let edited = source.replace("x+1u32", "x+2u32");
        // Every success must come from ordinary persistence: normalization is
        // absent in this fresh session, and the original source bodies are gone.
        let mut fresh = signature_session(store.clone(), &edited, false);
        for (name, fingerprint) in names.iter().zip(fingerprints) {
            let path = definition(name);
            let actual = fresh.item_signature(&path).unwrap();
            assert_eq!(actual.value.fingerprint, fingerprint);
            assert_eq!(fresh.recompute_count(&path, QueryKind::Signature), 0);
            let syntax = fresh.parse_definition(&path).unwrap().value;
            let resolution = fresh.resolver_snapshot(&path, QueryKind::Signature).unwrap();
            assert!(Arc::ptr_eq(&syntax, &resolution.syntax));
        }
        assert_eq!(fresh.owner_representation(&owner, &owner).unwrap().representation_revision, shape);
        let private_edit = edited.replace("value:u32", "value:u64");
        let mut invalid = signature_session(store.clone(), &private_edit, false);
        assert!(invalid.item_signature(&owner).is_err(), "private declaration changes cannot reuse an old owner view");
        let mut changed = signature_session(store, &private_edit, true);
        let actual = changed.owner_representation(&owner, &owner).unwrap().representation_revision;
        let mut cold = signature_session(Arc::new(store::NoneStore), &private_edit, true);
        assert_eq!(actual, cold.owner_representation(&owner, &owner).unwrap().representation_revision);
        assert_ne!(actual, shape);
    });
}
#[test]
fn signature_store_keeps_negative_namespace_and_relocated_diagnostic_dependencies() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source = "fn bad(x:Missing)->u32{0u32}\n";
        let path = definition("bad");
        let mut seed = signature_session(store.clone(), source, true);
        assert!(matches!(seed.item_signature(&path), Err(QueryError::Diagnostics(_))));
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        let QueryError::Diagnostics(actual) = fresh.item_signature(&path).unwrap_err() else { panic!("stored diagnostic missing") };
        assert_eq!(fresh.recompute_count(&path, QueryKind::Signature), 0);
        let mut cold = signature_session(Arc::new(store::NoneStore), source, true);
        let QueryError::Diagnostics(expected) = cold.item_signature(&path).unwrap_err() else { panic!() };
        assert_eq!(actual.iter().map(|d| d.render_human(&fresh.sources)).collect::<Vec<_>>(), expected.iter().map(|d| d.render_human(&cold.sources)).collect::<Vec<_>>());
        // Relative offsets are explicit inputs. Changing them must recompute
        // diagnostic positions; changing FileIds/anchors above must not.
        let shifted = format!("\n\n{source}");
        let mut positioned = signature_session(store.clone(), &shifted, true);
        let QueryError::Diagnostics(actual) = positioned.item_signature(&path).unwrap_err() else { panic!() };
        assert_eq!(positioned.recompute_count(&path, QueryKind::Signature), 1);
        let mut cold = signature_session(Arc::new(store::NoneStore), &shifted, true);
        let QueryError::Diagnostics(expected) = cold.item_signature(&path).unwrap_err() else { panic!() };
        assert_eq!(actual.iter().map(|d| d.render_human(&positioned.sources)).collect::<Vec<_>>(), expected.iter().map(|d| d.render_human(&cold.sources)).collect::<Vec<_>>());
        let repaired = format!("{source}type Missing=u32\n");
        let mut fresh = signature_session(store, &repaired, true);
        assert!(fresh.item_signature(&path).is_ok());
        assert_eq!(fresh.recompute_count(&path, QueryKind::Signature), 1);
    });
}

#[test]
fn lower_store_hits_validate_reads_and_v_before_demanding_modes_bodies() {
    with_compiler_stack(|| {
        let store = Arc::new(Observed::default());
        let source = "fn callee()->u32{1u32}\nfn caller()->u32{callee()}\n";
        let path = definition("caller");
        let mut seed = signature_session(store.clone(), source, true);
        let expected = seed.lower_to_tir(&path).unwrap();
        let expected_signature = expected.value.signature_hash;
        drop(seed);
        store.mode_reads.store(0, Ordering::Relaxed);
        let mut fresh = signature_session(store.clone(), source, true);
        fresh.adapters.modes = None;
        let actual = fresh.lower_to_tir(&path).unwrap();
        assert_eq!(actual.value.signature_hash, expected_signature);
        assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);
        assert_eq!(fresh.recompute_count(&path, QueryKind::Lower), 0);
        assert_eq!(store.mode_reads.load(Ordering::Relaxed), 0);
        let repeated = fresh.lower_to_tir(&path).unwrap();
        assert!(Arc::ptr_eq(&actual.value, &repeated.value));
        let changed = source.replace("callee()->u32{1u32}", "callee()->u64{1u64}");
        store.forbid_content_reads.store(true, Ordering::Relaxed);
        let mut invalid = signature_session(store.clone(), &changed, true);
        let rejected = invalid.lower_to_tir(&path).unwrap_err();
        store.forbid_content_reads.store(false, Ordering::Relaxed);
        let mut cold = signature_session(Arc::new(store::NoneStore), &changed, true);
        let expected = cold.lower_to_tir(&path).unwrap_err();
        let render = |engine: &QueryEngine, error| match error {
            QueryError::Diagnostics(ds) => ds.iter().map(|d| d.render_human(&engine.sources)).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(render(&invalid, rejected), render(&cold, expected));
    });
}
