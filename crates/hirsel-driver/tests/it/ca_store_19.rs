//! Durable completion consumes compact facts without loading the Modes body.
use hirsel_driver::*;
use hirsel_engine::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[test]
fn ordinary_program_checks_publish_bounded_batches_before_returning() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("results");
        let manifest = dir.path().join("hirsel.json");
        let source = (0..32)
            .map(|n| format!("pub type T{n} {{ value: u8 }}\npub fn f{n}(x:T{n})->T{n}{{x}}\n"))
            .collect::<String>();
        std::fs::write(dir.path().join("main.hir"), source).unwrap();
        std::fs::write(
            &manifest,
            serde_json::json!({
                "schema": 1,
                "package": {"name": "packcheck", "version": "1"},
                "sources": [{"path": "main.hir", "module": []}],
                "dependencies": [],
            })
            .to_string(),
        )
        .unwrap();
        let observed = Arc::new(store::DirStore::new(root.clone()));
        let statistics = store::ResultStore::local_product_store(&*observed)
            .unwrap()
            .enable_statistics();
        let compiler = |store: Arc<store::DirStore>| {
            COMPILER
                .clone()
                .with_owned_engines()
                .with_config(CompileConfig {
                    store,
                    jobs: hirsel_intern::jobs::Jobs::new(1),
                    ..CompileConfig::for_memory_cap(None)
                })
        };
        let checked = compiler(observed.clone()).check_path(&manifest).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let segments = std::fs::read_dir(root.join("packs"))
            .unwrap()
            .flatten()
            .flat_map(|shard| std::fs::read_dir(shard.path()).unwrap().flatten())
            .count();
        assert!(
            segments <= 8,
            "ordinary check published {segments} small segments"
        );
        assert_eq!(statistics.snapshot().counters["FileSyncs"], 0);
        assert_eq!(statistics.snapshot().counters["DirectorySyncs"], 0);
        assert_eq!(statistics.snapshot().counters["IndexPublications"], 1);
        assert_eq!(statistics.snapshot().counters["FinishCommits"], 1);
        assert_eq!(statistics.snapshot().counters["DropCommits"], 0);
        assert_eq!(statistics.snapshot().counters["CollectionCommits"], 0);
        // A separately opened provider must see the completed request while the
        // compiler and its original provider are still available to the caller.
        eprintln!(
            "physical-store-statistics: {}",
            serde_json::to_string(&statistics.snapshot()).unwrap()
        );
        let mut fresh = compiler(Arc::new(store::DirStore::new(root)))
            .load_check_package(&manifest)
            .unwrap();
        assert_eq!(fresh.engine.work_stats().parsed_bytes, 0);
        let warm = fresh.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(warm.completion, checked.completion);
        assert_eq!(warm.diagnostics, checked.diagnostics);
    });
}

#[derive(Default)]
struct Observed {
    inner: store::MemStore,
    reads: AtomicUsize,
    writes: AtomicUsize,
    parse_writes: AtomicUsize,
    content_reads: AtomicUsize,
    content_writes: AtomicUsize,
    mode_reads: AtomicUsize,
    summary_reads: AtomicUsize,
    summary_content_reads: AtomicUsize,
    indexes: Mutex<std::collections::BTreeSet<hirsel_syntax::ContentHash>>,
    forbid_content_reads: AtomicBool,
}

#[test]
fn ordinary_catalog_native_roots_survive_fresh_warm_checks() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("hirsel.json");
        std::fs::write(dir.path().join("main.hir"), "pub fn main()->u8{0u8}\n").unwrap();
        std::fs::write(
            &manifest,
            serde_json::json!({
                "schema": 1,
                "package": {"name": "warmroot", "version": "1"},
                "sources": [{"path": "main.hir", "module": []}],
                "dependencies": [],
                "entry": {"module": [], "name": "main"},
            })
            .to_string(),
        )
        .unwrap();
        let compiler = || {
            COMPILER
                .clone()
                .with_owned_engines()
                .with_config(CompileConfig {
                    store: Arc::new(store::DirStore::new(dir.path().join("results"))),
                    jobs: hirsel_intern::jobs::Jobs::new(1),
                    ..CompileConfig::for_memory_cap(None)
                })
        };
        let mut seed = compiler().load_check_package(&manifest).unwrap();
        let checked = seed.check(LoweringPolicy::Reachable).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty());
        let input = seed.engine.export_native_input().unwrap();
        assert!(input.definitions.contains_key(&seed.entry));
        let expected: Vec<_> = input.definitions.keys().cloned().collect();
        seed.engine.import_native_input(input).unwrap();
        drop(seed);

        let mut warm = compiler().load_check_package(&manifest).unwrap();
        assert_eq!(warm.engine.work_stats().parsed_bytes, 0);
        let checked = warm.check(LoweringPolicy::Reachable).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty());
        let input = warm.engine.export_native_input().unwrap();
        assert!(
            input.definitions.contains_key(&warm.entry),
            "warm catalog entry omitted from native roots"
        );
        assert_eq!(
            input.definitions.keys().cloned().collect::<Vec<_>>(),
            expected
        );
        warm.engine.import_native_input(input).unwrap();
    });
}
impl store::ResultStore for Observed {
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if key.2 == QueryKind::Modes {
            self.mode_reads.fetch_add(1, Ordering::Relaxed);
        }
        if key.2 == QueryKind::Summary {
            self.summary_reads.fetch_add(1, Ordering::Relaxed);
        }
        store::ResultStore::get(&self.inner, key)
    }
    fn put(&self, key: &store::EntryKey, bytes: &[u8]) {
        self.writes.fetch_add(1, Ordering::Relaxed);
        if key.2 == QueryKind::Parse {
            self.parse_writes.fetch_add(1, Ordering::Relaxed);
        }
        self.indexes.lock().unwrap().insert(key.0);
        store::ResultStore::put(&self.inner, key, bytes);
    }
    fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
        assert!(
            !self.forbid_content_reads.load(Ordering::Relaxed)
                || ![QueryKind::Modes, QueryKind::Summary, QueryKind::Lower]
                    .iter()
                    .any(|kind| reference.domain.kind == *kind as u32 + 16),
            "stale dependency action materialized a payload"
        );
        self.content_reads.fetch_add(1, Ordering::Relaxed);
        if reference.domain.kind == QueryKind::Summary as u32 + 16 {
            self.summary_content_reads.fetch_add(1, Ordering::Relaxed);
        }
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
            assert_eq!(
                fresh.store_adoptions(),
                4,
                "two signatures and two completions"
            );
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
            assert_eq!(
                count, 7,
                "93611be0 adds one parsed-source domain to the six per-definition addresses"
            );
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
            store.parse_writes.store(0, Ordering::Relaxed);
            let (reverted, outputs) = publish(store.clone(), source, jobs);
            assert!(outputs.iter().all(|o| o.result.is_ok()));
            assert_eq!(
                reverted.store_adoptions(),
                4,
                "two signatures and two completions"
            );
            assert_eq!(
                store.writes.load(Ordering::Relaxed),
                store.parse_writes.load(Ordering::Relaxed),
                "original semantic actions remain available; 93611be0 refreshes the single parsed-source slot"
            );
        }
    });
}

fn signature_session(
    store: Arc<dyn store::ResultStore>,
    source: &str,
    normalize: bool,
) -> QueryEngine {
    let mut adapters = compiler_adapters();
    if !normalize {
        adapters.signature = None;
    }
    let mut engine = QueryEngine::new(default_environment(), adapters).with_result_store(store);
    if !normalize {
        engine
            .sources
            .add("earlier.hir", "fn unrelated(){}\n".into())
            .unwrap();
    }
    let file = engine.sources.add("signatures.hir", source.into()).unwrap();
    assert!(
        engine
            .parse_file(file, &default_context())
            .unwrap()
            .diagnostics
            .is_empty()
    );
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
        let fingerprints: Vec<_> = names
            .iter()
            .map(|name| {
                seed.item_signature(&definition(name))
                    .unwrap()
                    .value
                    .fingerprint
            })
            .collect();
        let owner = definition("Hidden");
        let shape = seed
            .owner_representation(&owner, &owner)
            .unwrap()
            .representation_revision;
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
            let resolution = fresh
                .resolver_snapshot(&path, QueryKind::Signature)
                .unwrap();
            assert!(Arc::ptr_eq(&syntax, &resolution.syntax));
        }
        assert_eq!(
            fresh
                .owner_representation(&owner, &owner)
                .unwrap()
                .representation_revision,
            shape
        );
        let private_edit = edited.replace("value:u32", "value:u64");
        let mut invalid = signature_session(store.clone(), &private_edit, false);
        assert!(
            invalid.item_signature(&owner).is_err(),
            "private declaration changes cannot reuse an old owner view"
        );
        let mut changed = signature_session(store, &private_edit, true);
        let actual = changed
            .owner_representation(&owner, &owner)
            .unwrap()
            .representation_revision;
        let mut cold = signature_session(Arc::new(store::NoneStore), &private_edit, true);
        assert_eq!(
            actual,
            cold.owner_representation(&owner, &owner)
                .unwrap()
                .representation_revision
        );
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
        assert!(matches!(
            seed.item_signature(&path),
            Err(QueryError::Diagnostics(_))
        ));
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        let QueryError::Diagnostics(actual) = fresh.item_signature(&path).unwrap_err() else {
            panic!("stored diagnostic missing")
        };
        assert_eq!(fresh.recompute_count(&path, QueryKind::Signature), 0);
        let mut cold = signature_session(Arc::new(store::NoneStore), source, true);
        let QueryError::Diagnostics(expected) = cold.item_signature(&path).unwrap_err() else {
            panic!()
        };
        assert_eq!(
            actual
                .iter()
                .map(|d| d.render_human(&fresh.sources))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|d| d.render_human(&cold.sources))
                .collect::<Vec<_>>()
        );
        // Relative offsets are explicit inputs. Changing them must recompute
        // diagnostic positions; changing FileIds/anchors above must not.
        let shifted = format!("\n\n{source}");
        let mut positioned = signature_session(store.clone(), &shifted, true);
        let QueryError::Diagnostics(actual) = positioned.item_signature(&path).unwrap_err() else {
            panic!()
        };
        assert_eq!(positioned.recompute_count(&path, QueryKind::Signature), 1);
        let mut cold = signature_session(Arc::new(store::NoneStore), &shifted, true);
        let QueryError::Diagnostics(expected) = cold.item_signature(&path).unwrap_err() else {
            panic!()
        };
        assert_eq!(
            actual
                .iter()
                .map(|d| d.render_human(&positioned.sources))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|d| d.render_human(&cold.sources))
                .collect::<Vec<_>>()
        );
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
            QueryError::Diagnostics(ds) => ds
                .iter()
                .map(|d| d.render_human(&engine.sources))
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(render(&invalid, rejected), render(&cold, expected));
    });
}

#[test]
fn staged_registration_queries_restore_declarations_without_analyzed_bodies() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source = "fn value[n:size]()->u64{comptime (n*n)}\n";
        let path = definition("value");
        let mut seed = signature_session(store.clone(), source, true);
        seed.register_staged_definition(&path).unwrap();
        let lowered = seed.lower_to_tir(&path).unwrap();
        let snapshot = seed.signature_snapshot();
        let declarations = lowered
            .value
            .bodies
            .iter()
            .flat_map(|body| &body.rvalues)
            .filter_map(|value| match value {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => {
                    let signature = snapshot
                        .signature(hirsel_syntax::DefId::intern(definition))
                        .unwrap()
                        .unwrap();
                    Some((definition.clone(), signature.fingerprint))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(declarations.len(), 1);
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.register_staged_definition(&path).unwrap();
        assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);
        for (path, fingerprint) in &declarations {
            assert_eq!(
                fresh.item_signature(path).unwrap().value.fingerprint,
                *fingerprint
            );
        }
        let changed = source.replace("->u64", "->u32");
        let mut invalid = signature_session(store, &changed, true);
        invalid.adapters.modes = None;
        assert!(invalid.register_staged_definition(&path).is_err());
    });
}

#[test]
fn lower_producer_records_cannot_mint_or_transfer_verifier_authority() {
    // A remote/custom producer can replay every ordinary byte. Its default
    // provider interface supplies no verifier capability.
    struct ProducerOnly(Arc<store::MemStore>);
    impl store::ResultStore for ProducerOnly {
        fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
            store::ResultStore::get(&*self.0, key)
        }
        fn put(&self, key: &store::EntryKey, bytes: &[u8]) {
            store::ResultStore::put(&*self.0, key, bytes)
        }
        fn name(&self) -> &'static str {
            "producer-only"
        }
    }
    with_compiler_stack(|| {
        let source = "fn value()->u32{42u32}\n";
        let path = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut writer = signature_session(store.clone(), source, true);
        writer.lower_to_tir(&path).unwrap();
        assert_eq!(writer.body_validation_runs(&path), 1);
        drop(writer);
        let mut local = signature_session(store.clone(), source, false);
        local.adapters.modes = None;
        local.lower_to_tir(&path).unwrap();
        assert_eq!(
            local.body_validation_runs(&path),
            0,
            "local verifier receipt"
        );
        let mut remote = signature_session(Arc::new(ProducerOnly(store.clone())), source, false);
        remote.adapters.modes = None;
        remote.lower_to_tir(&path).unwrap();
        assert_eq!(remote.recompute_count(&path, QueryKind::Modes), 0);
        assert_eq!(
            remote.body_validation_runs(&path),
            1,
            "producer bytes require first V"
        );
        remote.lower_to_tir(&path).unwrap();
        assert_eq!(
            remote.body_validation_runs(&path),
            1,
            "this process now owns exact evidence"
        );
        // Copying all producer records into a new trusted cache transfers no
        // receipts, even when their content came from a verified compiler.
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
        }
        let mut copied = signature_session(copied, source, false);
        copied.adapters.modes = None;
        copied.lower_to_tir(&path).unwrap();
        assert_eq!(copied.body_validation_runs(&path), 1);
    });
}

#[test]
fn private_evaluation_uses_independent_receipts_in_scalar_and_worker_sessions() {
    let run = || {
        with_compiler_stack(|| {
            use hirsel_intern::timings::{self, Count};
            timings::enable();
            let runs = || {
                timings::report()
                    .counts
                    .into_iter()
                    .find(|(kind, _)| *kind == Count::VRuns)
                    .unwrap()
                    .1
            };
            let source = "const A:u8=comptime 7u8\nconst B:u8=comptime 8u8\n";
            let paths = vec![definition("A"), definition("B")];
            for jobs in [1, 2, 4, 8] {
                let store = Arc::new(store::MemStore::default());
                let mut seed = signature_session(store.clone(), source, true);
                let before = runs();
                let expected =
                    seed.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
                assert!(expected.iter().all(Result::is_ok));
                assert_eq!(runs() - before, 2, "first private V at jobs={jobs}");
                let mut fresh = signature_session(store.clone(), source, true);
                let before = runs();
                let actual =
                    fresh.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
                assert!(actual.iter().all(Result::is_ok));
                assert_eq!(
                    actual
                        .iter()
                        .map(|v| &v.as_ref().unwrap().root.kind)
                        .collect::<Vec<_>>(),
                    expected
                        .iter()
                        .map(|v| &v.as_ref().unwrap().root.kind)
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    runs(),
                    before,
                    "verifier-owned private receipts at jobs={jobs}"
                );
                let changed = source.replace("7u8", "9u8");
                let mut edited = signature_session(store, &changed, true);
                let before = runs();
                assert_eq!(
                    edited.materialize_constant(&definition("A")).unwrap(),
                    hirsel_tir::Constant::Integer(9)
                );
                assert_eq!(runs() - before, 1, "changed private artifact requires V");
            }
            "private-receipts".to_owned()
        })
    };
    if let Some(outputs) = hirsel_intern::under_salts(
        "ca_store_19::private_evaluation_uses_independent_receipts_in_scalar_and_worker_sessions",
        &[1],
        run,
    ) {
        assert_eq!(outputs, ["private-receipts"]);
    }
}

#[test]
fn scalar_and_staged_values_from_verify_off_require_private_admission() {
    with_compiler_stack(|| {
        let mut scalar = signature_session(
            Arc::new(store::MemStore::default()),
            "const A:u8=comptime 7u8\n",
            true,
        );
        scalar.verify = Verify::Off;
        assert_eq!(
            scalar.materialize_constant(&definition("A")).unwrap(),
            hirsel_tir::Constant::Integer(7)
        );
        scalar.verify = Verify::On;
        scalar.adapters.lowering = None;
        assert!(
            scalar.materialize_constant(&definition("A")).is_err(),
            "an unverified private value must demand its initializer"
        );

        let mut staged = signature_session(
            Arc::new(store::MemStore::default()),
            "fn value[n:size]()->u64{comptime (n*n)}\n",
            true,
        );
        staged.verify = Verify::Off;
        let lowered = staged.lower_to_tir(&definition("value")).unwrap();
        let hidden = lowered
            .value
            .bodies
            .iter()
            .flat_map(|b| &b.rvalues)
            .find_map(|v| match v {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        hirsel_tir::TirProvider::constant_instance(&mut staged, &hidden, &[2]).unwrap();
        staged.verify = Verify::On;
        staged.adapters.lowering = None;
        assert!(
            hirsel_tir::TirProvider::constant_instance(&mut staged, &hidden, &[2]).is_err(),
            "a staged VerifyOff value must demand private admission"
        );
    });
}

#[test]
fn scalar_evaluation_products_require_local_completion_and_skip_analyzed_bodies() {
    with_compiler_stack(|| {
        let source = "fn helper()->u32{7u32}\nconst VALUE:u32=comptime helper()\n";
        let path = definition("VALUE");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        for name in ["helper", "VALUE"] {
            seed.item_signature(&definition(name)).unwrap();
        }
        assert_eq!(
            seed.materialize_constant(&path).unwrap(),
            hirsel_tir::Constant::Integer(7)
        );
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        assert_eq!(
            fresh.materialize_constant(&path).unwrap(),
            hirsel_tir::Constant::Integer(7)
        );
        assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);
        assert!(fresh.store_adoptions() > 0);

        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut untrusted = signature_session(copied, source, false);
        untrusted.adapters.modes = None;
        untrusted.adapters.lowering = None;
        assert!(
            untrusted.materialize_constant(&path).is_err(),
            "copied values cannot supply private V completion"
        );
        let mut changed = signature_session(store, &source.replace("7u32", "9u32"), true);
        assert_eq!(
            changed.materialize_constant(&path).unwrap(),
            hirsel_tir::Constant::Integer(9)
        );
    });
}

#[test]
fn worker_evaluation_products_adopt_only_local_complete_verdicts() {
    with_compiler_stack(|| {
        let source =
            "fn helper()->u32{7u32}\nconst A:u32=comptime helper()\nconst B:u32=comptime A+1u32\n";
        let paths = vec![definition("A"), definition("B")];
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            for name in ["helper", "A", "B"] {
                seed.item_signature(&definition(name)).unwrap();
            }
            let expected =
                seed.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(expected.iter().all(Result::is_ok), "{expected:?}");
            let mut fresh = signature_session(store.clone(), source, false);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            let actual =
                fresh.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(actual.iter().all(Result::is_ok), "jobs={jobs}: {actual:?}");
            assert_eq!(
                actual
                    .iter()
                    .map(|v| &v.as_ref().unwrap().root.kind)
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|v| &v.as_ref().unwrap().root.kind)
                    .collect::<Vec<_>>()
            );
            for path in &paths {
                assert_eq!(fresh.recompute_count(path, QueryKind::Modes), 0);
            }
            let copied = Arc::new(store::MemStore::default());
            for (key, bytes) in store.take() {
                store::ResultStore::put(&*copied, &key, &bytes);
                store::ResultStore::put(&*store, &key, &bytes);
            }
            let mut untrusted = signature_session(copied, source, false);
            untrusted.adapters.modes = None;
            untrusted.adapters.lowering = None;
            assert!(
                untrusted
                    .materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs))
                    .iter()
                    .all(Result::is_err)
            );
            let mut edited = signature_session(store, &source.replace("7u32", "9u32"), true);
            assert_eq!(
                edited.materialize_constant(&definition("B")).unwrap(),
                hirsel_tir::Constant::Integer(10)
            );
        }
    });
}

#[test]
fn comptime_lower_products_need_local_execution_completion_before_body_free_hits() {
    with_compiler_stack(|| {
        for source in [
            "fn helper()->u32{7u32}\nfn value()->u32{comptime helper()}\n",
            "fn value()->u32{comptime 7u32}\n",
        ] {
            let path = definition("value");
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            seed.item_signature(&path).unwrap();
            seed.lower_to_tir(&path).unwrap();
            let mut fresh = signature_session(store.clone(), source, false);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            let adopted = fresh.lower_to_tir(&path).unwrap();
            assert_eq!(adopted.value.path, hirsel_syntax::DefId::intern(&path));
            assert_eq!(fresh.lower_adapter_calls(&path), 0);
            assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);
            let copied = Arc::new(store::MemStore::default());
            for (key, bytes) in store.take() {
                store::ResultStore::put(&*copied, &key, &bytes);
                store::ResultStore::put(&*store, &key, &bytes);
            }
            let mut remote = signature_session(copied, source, false);
            remote.adapters.modes = None;
            remote.adapters.lowering = None;
            assert!(
                remote.lower_to_tir(&path).is_err(),
                "producer Lower bytes cannot supply private execution authority"
            );
            let edited = source.replace("7u32", "9u32");
            let mut stale = signature_session(store.clone(), &edited, true);
            stale.adapters.modes = None;
            stale.adapters.lowering = None;
            assert!(stale.lower_to_tir(&path).is_err());
            let off_store = Arc::new(store::MemStore::default());
            let mut off = signature_session(off_store.clone(), source, true);
            off.verify = Verify::Off;
            off.lower_to_tir(&path).unwrap();
            let mut on = signature_session(off_store, source, true);
            on.adapters.modes = None;
            on.adapters.lowering = None;
            assert!(
                on.lower_to_tir(&path).is_err(),
                "VerifyOff cannot mint completion"
            );
        }
    });
}

#[test]
fn resident_lower_completion_rechecks_its_private_validation_context() {
    with_compiler_stack(|| {
        let source =
            "pub opaque type Hidden:plain uses {}{field:u32}\nfn value()->u32{comptime 7u32}\n";
        let store = Arc::new(store::MemStore::default());
        let mut engine = signature_session(store, source, true);
        let path = definition("value");
        engine.lower_to_tir(&path).unwrap();
        let before = engine.lower_adapter_calls(&path);
        engine.lower_to_tir(&path).unwrap();
        assert_eq!(engine.lower_adapter_calls(&path), before);
        let file = engine
            .sources
            .add("signatures.hir", source.replace("field:u32", "field:u64"))
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        engine.lower_to_tir(&path).unwrap();
        assert_eq!(
            engine.lower_adapter_calls(&path),
            before + 1,
            "complete private V context changed"
        );
    });
}

#[test]
fn staged_evaluation_products_bind_arguments_and_local_completion_before_body_demand() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "fn value[n:size]()->u64{comptime (n*n)}\n";
        let owner = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let lowered = seed.lower_to_tir(&owner).unwrap();
        let hidden = lowered
            .value
            .bodies
            .iter()
            .flat_map(|b| &b.rvalues)
            .find_map(|v| match v {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        let expected: Vec<_> = [2, 3]
            .into_iter()
            .map(|n| seed.constant_instance(&hidden, &[n]).unwrap())
            .collect();
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        fresh.register_staged_definition(&owner).unwrap();
        for (n, expected) in [2, 3].into_iter().zip(&expected) {
            let actual = fresh.constant_instance(&hidden, &[n]).unwrap();
            assert_eq!(actual.root.kind, expected.root.kind, "selected size={n}");
        }
        assert_eq!(fresh.recompute_count(&owner, QueryKind::Modes), 0);
        assert!(
            fresh.constant_instance(&hidden, &[4]).is_err(),
            "unpublished instance cannot alias a selected size"
        );
        assert!(
            fresh.constant_instance(&hidden, &[2, 3]).is_err(),
            "arity remains checked"
        );

        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        remote.register_staged_definition(&owner).unwrap();
        assert!(
            remote.constant_instance(&hidden, &[2]).is_err(),
            "ordinary staged values cannot transfer private completion"
        );

        let edited = source.replace("n*n", "n+n");
        let mut changed = signature_session(store.clone(), &edited, true);
        changed.register_staged_definition(&owner).unwrap();
        changed.adapters.modes = None;
        changed.adapters.lowering = None;
        assert!(
            changed.constant_instance(&hidden, &[2]).is_err(),
            "owner body edits invalidate all selected instances"
        );

        let off_store = Arc::new(store::MemStore::default());
        let mut off = signature_session(off_store.clone(), source, true);
        off.verify = Verify::Off;
        off.register_staged_definition(&owner).unwrap();
        off.constant_instance(&hidden, &[2]).unwrap();
        let mut on = signature_session(off_store, source, true);
        on.register_staged_definition(&owner).unwrap();
        on.adapters.modes = None;
        on.adapters.lowering = None;
        assert!(
            on.constant_instance(&hidden, &[2]).is_err(),
            "VerifyOff staged products cannot mint private completion"
        );
    });
}

#[test]
fn worker_staged_products_publish_only_prefix_selected_instances() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\nconst FIRST:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nfn BAD()->u64{comptime {let zero=0u64;1u64/zero}}\nfn LATER()->u64{comptime {let xs:Inline[u8,3]=[1,2,3];scaled(xs)}}\n";
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            let lowered = seed.lower_to_tir(&definition("scaled")).unwrap();
            let declarations = lowered.value.bodies[lowered.value.entry.0 as usize]
                .rvalues
                .iter()
                .filter_map(|v| match v {
                    hirsel_tir::Rvalue::ReadConstant {
                        definition,
                        arguments,
                    } if !arguments.is_empty() => Some(definition.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(declarations.len(), 1, "one explicit staged expression");
            let hidden = declarations[0].clone();
            let outcomes = seed.check_lower_definition_prefix(
                vec![definition("FIRST"), definition("BAD"), definition("LATER")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert_eq!(outcomes.len(), 2, "jobs={jobs}");
            assert!(outcomes[0].result.is_ok());
            assert!(outcomes[1].result.is_err());
            let mut fresh = signature_session(store.clone(), source, true);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            fresh
                .register_staged_definition(&definition("scaled"))
                .unwrap();
            assert_eq!(
                fresh.constant_instance(&hidden, &[2]).unwrap().root.kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)),
                "jobs={jobs}"
            );
            assert!(
                fresh.constant_instance(&hidden, &[3]).is_err(),
                "later speculative size cannot publish at jobs={jobs}"
            );
            let mut oracle = signature_session(store, source, true)
                .with_verify_store(store::VerifyStore::Recompute);
            oracle
                .register_staged_definition(&definition("scaled"))
                .unwrap();
            assert_eq!(
                oracle.constant_instance(&hidden, &[2]).unwrap().root.kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4))
            );
        }
    });
}

#[test]
fn staged_lower_products_restore_own_declarations_before_admission_without_modes() {
    with_compiler_stack(|| {
        let source = "pub opaque type Hidden:plain uses {}{field:u32}\nfn value[n:size]()->u64{comptime (n*n)}\n";
        let path = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let expected = seed.lower_to_tir(&path).unwrap();
        let declaration = expected
            .value
            .bodies
            .iter()
            .flat_map(|body| &body.rvalues)
            .find_map(|value| match value {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        let signature = seed.item_signature(&declaration).unwrap().value.fingerprint;
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        let actual = fresh.lower_to_tir(&path).unwrap();
        assert_eq!(actual.value.path, expected.value.path);
        assert_eq!(
            fresh
                .item_signature(&declaration)
                .unwrap()
                .value
                .fingerprint,
            signature
        );
        assert_eq!(fresh.lower_adapter_calls(&path), 0);
        assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);

        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        assert!(
            remote.lower_to_tir(&path).is_err(),
            "declaration bytes cannot transfer V completion"
        );
        let mut edited = signature_session(
            store.clone(),
            &source.replace("field:u32", "field:u64"),
            true,
        );
        edited.adapters.modes = None;
        edited.adapters.lowering = None;
        assert!(
            edited.lower_to_tir(&path).is_err(),
            "private V universe changes invalidate completion"
        );
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        oracle.lower_to_tir(&path).unwrap();
    });
}

#[test]
fn enclosing_evaluations_replay_exact_staged_selections_before_body_free_adoption() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime A+1u64\n";
        let roots = vec![definition("A"), definition("B")];
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            let lower = seed.lower_to_tir(&definition("scaled")).unwrap();
            let hidden = lower.value.bodies[lower.value.entry.0 as usize]
                .rvalues
                .iter()
                .find_map(|r| match r {
                    hirsel_tir::Rvalue::ReadConstant {
                        definition,
                        arguments,
                    } if !arguments.is_empty() => Some(definition.clone()),
                    _ => None,
                })
                .unwrap();
            let expected =
                seed.materialize_values(roots.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(
                expected.iter().all(Result::is_ok),
                "jobs={jobs}: {expected:?}"
            );
            let mut fresh = signature_session(store.clone(), source, false);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            let actual =
                fresh.materialize_values(roots.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(actual.iter().all(Result::is_ok), "jobs={jobs}: {actual:?}");
            assert_eq!(
                actual
                    .iter()
                    .map(|v| &v.as_ref().unwrap().root.kind)
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|v| &v.as_ref().unwrap().root.kind)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                fresh.constant_instance(&hidden, &[2]).unwrap().root.kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4))
            );
            assert!(
                fresh.constant_instance(&hidden, &[3]).is_err(),
                "only selected instances replay"
            );
            for root in &roots {
                assert_eq!(fresh.recompute_count(root, QueryKind::Modes), 0);
            }
            let copied = Arc::new(store::MemStore::default());
            for (key, bytes) in store.take() {
                store::ResultStore::put(&*copied, &key, &bytes);
                store::ResultStore::put(&*store, &key, &bytes);
            }
            let mut remote = signature_session(copied, source, false);
            remote.adapters.modes = None;
            remote.adapters.lowering = None;
            assert!(
                remote
                    .materialize_values(roots.clone(), hirsel_intern::jobs::Jobs::new(jobs))
                    .iter()
                    .all(Result::is_err),
                "producer transactions cannot transfer private completion"
            );
            let mut oracle = signature_session(store.clone(), source, true)
                .with_verify_store(store::VerifyStore::Recompute);
            oracle
                .register_staged_definition(&definition("scaled"))
                .unwrap();
            assert_eq!(
                oracle.materialize_constant(&definition("B")).unwrap(),
                hirsel_tir::Constant::Integer(5),
                "worker/scalar transaction encoding jobs={jobs}"
            );
            let mut changed = signature_session(store, &source.replace("n*n", "n+n"), true);
            changed.adapters.modes = None;
            changed.adapters.lowering = None;
            assert!(
                changed.materialize_constant(&definition("B")).is_err(),
                "stage owner body changes invalidate enclosing values"
            );
        }
    });
}

#[test]
fn lower_hits_replay_exact_staged_values_and_preserve_completion_authority() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\nfn value()->u64{comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}}\n";
        let root = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let generic = seed.lower_to_tir(&definition("scaled")).unwrap();
        let hidden = generic.value.bodies[generic.value.entry.0 as usize]
            .rvalues
            .iter()
            .find_map(|r| match r {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        let expected = seed.lower_to_tir(&root).unwrap();
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        let actual = fresh.lower_to_tir(&root).unwrap();
        assert_eq!(actual.value.path, expected.value.path);
        assert_eq!(fresh.lower_adapter_calls(&root), 0);
        assert_eq!(fresh.recompute_count(&root, QueryKind::Modes), 0);
        assert_eq!(
            fresh.constant_instance(&hidden, &[2]).unwrap().root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4))
        );
        assert!(
            fresh.constant_instance(&hidden, &[3]).is_err(),
            "unselected size stays absent"
        );
        fresh.lower_to_tir(&root).unwrap();
        assert_eq!(
            fresh.lower_adapter_calls(&root),
            0,
            "resident Lower replays the same ledger"
        );
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        assert!(
            remote.lower_to_tir(&root).is_err(),
            "producer ledger cannot transfer V completion"
        );
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        oracle
            .register_staged_definition(&definition("scaled"))
            .unwrap();
        oracle.lower_to_tir(&root).unwrap();
    });
}

#[test]
fn generated_source_products_require_local_completion_and_skip_generator_execution() {
    with_compiler_stack(|| {
        let source = "pub opaque type Hidden:plain uses {}{field:u32}\nfn text()->String{\"fn()->u32 uses {}{7u32}\"}\nfn value()->u32 uses {}=comptime Ok(codegen.body(text()))\n";
        let path = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        seed.check_definition(&path).unwrap();
        let expected = seed.generated_source(&path).unwrap().text.clone();
        let mut fresh = signature_session(store.clone(), source, true);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        fresh.check_definition(&path).unwrap();
        assert_eq!(fresh.generated_source(&path).unwrap().text, expected);
        assert_eq!(
            fresh.lower_adapter_calls(&definition("text")),
            0,
            "generator helper was not executed"
        );
        assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, true);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        assert!(
            remote.check_definition(&path).is_err(),
            "producer text cannot transfer private generator completion"
        );
        let changed = source.replace("7u32", "9u32");
        let mut invalid = signature_session(store.clone(), &changed, true);
        invalid.adapters.modes = None;
        invalid.adapters.lowering = None;
        assert!(
            invalid.check_definition(&path).is_err(),
            "executed helper changes invalidate text"
        );
        let mut private = signature_session(
            store.clone(),
            &source.replace("field:u32", "field:u64"),
            true,
        );
        private.adapters.modes = None;
        private.adapters.lowering = None;
        assert!(
            private.check_definition(&path).is_err(),
            "private V universe changes invalidate text completion"
        );
        let off_store = Arc::new(store::MemStore::default());
        let mut off = signature_session(off_store.clone(), source, true);
        off.verify = Verify::Off;
        off.check_definition(&path).unwrap();
        let mut on = signature_session(off_store, source, true);
        on.adapters.modes = None;
        on.adapters.lowering = None;
        assert!(
            on.check_definition(&path).is_err(),
            "VerifyOff generator text cannot mint completion"
        );
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        oracle.check_definition(&path).unwrap();
        assert_eq!(oracle.generated_source(&path).unwrap().text, expected);
    });
}

#[test]
fn worker_generated_text_persists_only_admitted_roots_and_matches_scalar_records() {
    with_compiler_stack(|| {
        let source = "fn text()->String{\"fn()->u32 uses {}{7u32}\"}\nfn value()->u32 uses {}=comptime Ok(codegen.body(text()))\nconst FIRST:u32=comptime value()\nfn BAD()->u32{comptime {let zero=0u32;1u32/zero}}\nfn later()->u32 uses {}=comptime Ok(codegen.body(\"fn()->u32 uses {}{9u32}\"))\n";
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            let outcomes = seed.check_lower_definition_prefix(
                vec![definition("FIRST"), definition("BAD"), definition("later")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert_eq!(outcomes.len(), 2, "jobs={jobs}");
            assert!(
                outcomes[0].result.is_ok(),
                "jobs={jobs}: {:?}",
                outcomes[0].result
            );
            assert!(outcomes[1].result.is_err());
            assert!(
                seed.generated_source(&definition("later")).is_none(),
                "speculative suffix remains unpublished"
            );
            let expected = seed
                .generated_source(&definition("value"))
                .unwrap()
                .text
                .clone();
            let mut fresh = signature_session(store.clone(), source, true);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            fresh.check_definition(&definition("value")).unwrap();
            assert_eq!(
                fresh.generated_source(&definition("value")).unwrap().text,
                expected
            );
            assert!(
                fresh.check_definition(&definition("later")).is_err(),
                "suffix text not persisted at jobs={jobs}"
            );
            let mut oracle = signature_session(store, source, true)
                .with_verify_store(store::VerifyStore::Recompute);
            oracle.check_definition(&definition("value")).unwrap();
            assert_eq!(
                oracle.generated_source(&definition("value")).unwrap().text,
                expected
            );
        }
    });
}

#[test]
fn selected_generic_generators_reuse_ordinary_text_under_canonical_actuals() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "type Point{x:u32}\ntype Other{x:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{let _=s;Ok(codegen.body(\"fn(v:standalone@0/Point)->u64 uses {}{let _=v;12u64}\"))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\n";
        let path = definition("width");
        let arguments = |engine: &QueryEngine, name: &str| {
            vec![hirsel_types::GenericArgument::Type(engine.types().intern(
                hirsel_types::Type::with_kind(hirsel_types::TypeKind::Named {
                    definition: hirsel_syntax::DefId::intern(&definition(name)),
                    arguments: vec![].into(),
                }),
            ))]
        };
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let selected = seed
            .instance_body(&path, &arguments(&seed, "Point"))
            .unwrap()
            .unwrap();
        let expected = seed.generated_source(&selected).unwrap().text.clone();
        let mut fresh = signature_session(store.clone(), source, true);
        fresh.adapters.lowering = None;
        let adopted = fresh
            .instance_body(&path, &arguments(&fresh, "Point"))
            .unwrap()
            .unwrap();
        assert_eq!(adopted, selected);
        assert_eq!(fresh.generated_source(&adopted).unwrap().text, expected);
        assert_eq!(
            fresh.lower_adapter_calls(&definition("gen")),
            0,
            "no generic generator execution"
        );
        assert!(
            fresh
                .instance_body(&path, &arguments(&fresh, "Other"))
                .is_err(),
            "an unselected type has no cached generator result"
        );
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, true);
        remote.adapters.lowering = None;
        assert!(
            remote
                .instance_body(&path, &arguments(&remote, "Point"))
                .is_err(),
            "instance text cannot transfer private completion"
        );
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        assert_eq!(
            oracle
                .instance_body(&path, &arguments(&oracle, "Point"))
                .unwrap(),
            Some(selected)
        );
    });
}

#[test]
fn enclosing_products_replay_selected_generated_bodies_without_execution() {
    with_compiler_stack(|| {
        let source = "fn made()->u32 uses {}=comptime Ok(codegen.body(\"fn()->u32 uses {}{7u32}\"))\nconst A:u32=comptime made()\nconst B:u32=comptime A+1u32\nfn value()->u32{comptime made()}\n";
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        assert_eq!(
            seed.materialize_constant(&definition("B")).unwrap(),
            hirsel_tir::Constant::Integer(8)
        );
        seed.lower_to_tir(&definition("value")).unwrap();
        let text = seed
            .generated_source(&definition("made"))
            .unwrap()
            .text
            .clone();
        let mut fresh = signature_session(store.clone(), source, true);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        assert_eq!(
            fresh.materialize_constant(&definition("B")).unwrap(),
            hirsel_tir::Constant::Integer(8)
        );
        assert_eq!(
            fresh.generated_source(&definition("made")).unwrap().text,
            text
        );
        fresh.lower_to_tir(&definition("value")).unwrap();
        fresh.lower_to_tir(&definition("value")).unwrap();
        for name in ["A", "B", "value"] {
            assert_eq!(
                fresh.recompute_count(&definition(name), QueryKind::Modes),
                0
            );
            assert_eq!(fresh.lower_adapter_calls(&definition(name)), 0);
        }
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, true);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        assert!(remote.materialize_constant(&definition("B")).is_err());
        let mut oracle = signature_session(store.clone(), source, true)
            .with_verify_store(store::VerifyStore::Recompute);
        assert_eq!(
            oracle.materialize_constant(&definition("B")).unwrap(),
            hirsel_tir::Constant::Integer(8)
        );
        oracle.lower_to_tir(&definition("value")).unwrap();
        let mut changed = signature_session(store, &source.replace("7u32", "9u32"), true);
        changed.adapters.modes = None;
        changed.adapters.lowering = None;
        assert!(changed.materialize_constant(&definition("B")).is_err());
    });
}

#[test]
fn worker_enclosing_generated_values_replay_only_selected_canonical_instances() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "type Point{x:u32}\ntype Other{x:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{let _=s;Ok(codegen.body(\"fn(v:standalone@0/Point)->u64 uses {}{let _=v;12u64}\"))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\nconst A:u64=comptime width(Point(x:1u32))\nconst B:u64=comptime A+1u64\nfn value()->u64{comptime width(Point(x:2u32))}\n";
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            let roots = vec![definition("A"), definition("B")];
            let values =
                seed.materialize_values(roots.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(values.iter().all(Result::is_ok), "jobs={jobs}: {values:?}");
            seed.lower_to_tir(&definition("value")).unwrap();
            let selected: Vec<_> = seed
                .generated_sources()
                .map(|(path, source)| (path.clone(), source.text.clone()))
                .collect();
            assert_eq!(selected.len(), 1);
            let mut fresh = signature_session(store.clone(), source, true);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            let actual = fresh.materialize_values(roots, hirsel_intern::jobs::Jobs::new(jobs));
            assert!(actual.iter().all(Result::is_ok), "jobs={jobs}: {actual:?}");
            assert_eq!(
                actual
                    .iter()
                    .map(|v| &v.as_ref().unwrap().root.kind)
                    .collect::<Vec<_>>(),
                values
                    .iter()
                    .map(|v| &v.as_ref().unwrap().root.kind)
                    .collect::<Vec<_>>()
            );
            fresh.lower_to_tir(&definition("value")).unwrap();
            assert_eq!(
                fresh
                    .generated_sources()
                    .map(|(path, source)| (path.clone(), source.text.clone()))
                    .collect::<Vec<_>>(),
                selected
            );
            assert_eq!(fresh.lower_adapter_calls(&definition("gen")), 0);
            for name in ["A", "B", "value"] {
                assert_eq!(
                    fresh.recompute_count(&definition(name), QueryKind::Modes),
                    0
                );
            }
            let other = fresh.types().intern(hirsel_types::Type::with_kind(
                hirsel_types::TypeKind::Named {
                    definition: hirsel_syntax::DefId::intern(&definition("Other")),
                    arguments: vec![].into(),
                },
            ));
            assert!(
                fresh
                    .instance_body(
                        &definition("width"),
                        &[hirsel_types::GenericArgument::Type(other)]
                    )
                    .is_err(),
                "unselected Other has no completed text"
            );
            let copied = Arc::new(store::MemStore::default());
            for (key, bytes) in store.take() {
                store::ResultStore::put(&*copied, &key, &bytes);
                store::ResultStore::put(&*store, &key, &bytes);
            }
            let mut remote = signature_session(copied, source, true);
            remote.adapters.modes = None;
            remote.adapters.lowering = None;
            assert!(remote.materialize_constant(&definition("B")).is_err());
            let mut oracle = signature_session(store.clone(), source, true)
                .with_verify_store(store::VerifyStore::Recompute);
            assert_eq!(
                oracle.materialize_constant(&definition("B")).unwrap(),
                hirsel_tir::Constant::Integer(13),
                "jobs={jobs}"
            );
            oracle.lower_to_tir(&definition("value")).unwrap();
            let mut changed = signature_session(store, &source.replace("12u64", "14u64"), true);
            changed.adapters.modes = None;
            changed.adapters.lowering = None;
            assert!(changed.materialize_constant(&definition("B")).is_err());
        }
    });
}

#[test]
fn ordinary_source_audits_reuse_compact_products_and_keep_private_completion() {
    with_compiler_stack(|| {
        let source = "fn helper()->u32{7u32}\nfn plain(x:u32)->u32{x+1u32}\nfn staged()->u32{comptime helper()}\n";
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let plain = seed.source_audit(&definition("plain")).unwrap();
        let staged = seed.source_audit(&definition("staged")).unwrap();
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        assert_eq!(fresh.source_audit(&definition("plain")).unwrap(), plain);
        assert_eq!(fresh.source_audit(&definition("staged")).unwrap(), staged);
        for name in ["plain", "staged"] {
            assert_eq!(
                fresh.recompute_count(&definition(name), QueryKind::Modes),
                0
            );
            assert_eq!(fresh.lower_adapter_calls(&definition(name)), 0);
        }
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        assert!(
            remote.source_audit(&definition("staged")).is_err(),
            "producer audit cannot transfer private execution completion"
        );
        let mut changed = signature_session(store.clone(), &source.replace("7u32", "9u32"), true);
        changed.adapters.modes = None;
        changed.adapters.lowering = None;
        assert!(
            changed.source_audit(&definition("staged")).is_err(),
            "executed helper invalidates audit substitutions"
        );
        let mut oracle =
            signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        assert_eq!(oracle.source_audit(&definition("plain")).unwrap(), plain);
        assert_eq!(oracle.source_audit(&definition("staged")).unwrap(), staged);
        let off_store = Arc::new(store::MemStore::default());
        let mut off = signature_session(off_store.clone(), source, true);
        off.verify = Verify::Off;
        off.source_audit(&definition("staged")).unwrap();
        let mut on = signature_session(off_store, source, true);
        on.adapters.modes = None;
        on.adapters.lowering = None;
        assert!(on.source_audit(&definition("staged")).is_err());
    });
}

#[test]
fn semantic_body_products_reuse_body_free_nodes_and_keep_execution_dependencies() {
    with_compiler_stack(|| {
        let source = "fn helper()->u32{7u32}\nfn root()->u32{helper()}\nfn staged()->u32{comptime helper()}\n";
        let roots = [definition("root"), definition("staged")];
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        let expected: Vec<_> = roots
            .iter()
            .map(|root| seed.semantic_program_revision(root).unwrap())
            .collect();
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        for (root, expected) in roots.iter().zip(&expected) {
            assert_eq!(&fresh.semantic_program_revision(root).unwrap(), expected);
            assert_eq!(fresh.recompute_count(root, QueryKind::Modes), 0);
            assert_eq!(fresh.lower_adapter_calls(root), 0);
        }
        let mut oracle = signature_session(store.clone(), source, true)
            .with_verify_store(store::VerifyStore::Recompute);
        for (root, expected) in roots.iter().zip(&expected) {
            assert_eq!(&oracle.semantic_program_revision(root).unwrap(), expected);
        }
        let changed_source = source.replace("7u32", "9u32");
        let mut changed = signature_session(store.clone(), &changed_source, false);
        changed.adapters.modes = None;
        changed.adapters.lowering = None;
        assert!(
            changed
                .semantic_program_revision(&definition("staged"))
                .is_err()
        );
        let mut reference =
            signature_session(Arc::new(store::MemStore::default()), &changed_source, true);
        assert_ne!(
            reference
                .semantic_program_revision(&definition("root"))
                .unwrap(),
            expected[0],
            "equal callable summaries do not cut off executed source content"
        );
    });
}

#[test]
fn ordinary_parse_heads_restore_names_and_signatures_without_syntax_or_type_tables() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source =
            "pub type Hidden = u32\nfn value(x:Hidden)->Hidden{x}\nfn unused()->u32{99u32}\n";
        let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = writer.sources.add("heads.hir", source.into()).unwrap();
        writer.index_source(file, &default_context()).unwrap();
        let path = |name: &str| {
            let mut p = entry_path(&default_context());
            p.0.name = name.into();
            p
        };
        let expected = writer
            .item_signature(&path("value"))
            .unwrap()
            .value
            .fingerprint;
        // Keep this fixture heads-only: the ordinary writer also publishes a
        // separate Parse output now. Its absence must still require a parser.
        let mut removed_syntax = 0;
        for (key, bytes) in store.take() {
            if key.2 == QueryKind::Parse && bytes.starts_with(&2u32.to_le_bytes()) {
                removed_syntax += 1;
            } else {
                store::ResultStore::put(&*store, &key, &bytes);
            }
        }
        assert_eq!(removed_syntax, 1, "remove exactly the source Parse output");
        let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        reader.adapters.parser = None;
        reader.adapters.lexer = None;
        reader.adapters.signature = None;
        reader.sources.add("padding.hir", "\n".into()).unwrap();
        let file = reader.sources.add("heads.hir", source.into()).unwrap();
        reader.index_source(file, &default_context()).unwrap();
        assert_eq!(reader.work_stats().parsed_bytes, 0);
        assert_eq!(
            reader
                .item_signature(&path("value"))
                .unwrap()
                .value
                .fingerprint,
            expected
        );
        assert_eq!(reader.work_stats().parsed_bytes, 0);
        assert_eq!(
            reader.recompute_count(&path("value"), QueryKind::Signature),
            0
        );
        assert!(
            reader.parse_definition(&path("unused")).is_err(),
            "a syntax demand must use the absent parser"
        );
        let mut oracle = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone())
            .with_verify_store(store::VerifyStore::Recompute);
        let file = oracle.sources.add("heads.hir", source.into()).unwrap();
        oracle.index_source(file, &default_context()).unwrap();
        assert!(oracle.work_stats().parsed_bytes > 0);
        assert_eq!(
            oracle
                .item_signature(&path("value"))
                .unwrap()
                .value
                .fingerprint,
            expected
        );
        let mut edited =
            QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store);
        edited.adapters.parser = None;
        let file = edited
            .sources
            .add("heads.hir", source.replace("Hidden = u32", "Hidden = u64"))
            .unwrap();
        assert!(
            edited.index_source(file, &default_context()).is_err(),
            "observed source bytes invalidate heads"
        );
    });
}

#[test]
fn ordinary_parse_heads_materialize_only_demanded_files_and_preserve_resident_edits() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source = "fn value()->u32{7u32}\n";
        let other = "fn other()->u32{3u32}\n";
        let path = |name: &str| {
            let mut p = entry_path(&default_context());
            p.0.name = name.into();
            p
        };
        let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        for (name, text) in [("heads.hir", source), ("other.hir", other)] {
            let file = writer.sources.add(name, text.into()).unwrap();
            writer.index_source(file, &default_context()).unwrap();
        }
        // V independently needs every signature in its universe. Supply the
        // ordinary header product so that this body-materialization control
        // does not force an unrelated source parse to normalize that header.
        writer.item_signature(&path("other")).unwrap();
        // The body-free reader also needs S5's module proof. A declaration
        // header alone cannot prove an unchanged sibling's private body.
        writer.signature_snapshot();
        // This control exercises materialization from heads without a Parse
        // output; keep its exact parser-work and edit assertions below.
        let mut removed_syntax = 0;
        for (key, bytes) in store.take() {
            if key.2 == QueryKind::Parse && bytes.starts_with(&2u32.to_le_bytes()) {
                removed_syntax += 1;
            } else {
                store::ResultStore::put(&*store, &key, &bytes);
            }
        }
        assert_eq!(removed_syntax, 2);
        let mut reader =
            QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store);
        for (name, text) in [("heads.hir", source), ("other.hir", other)] {
            let file = reader.sources.add(name, text.into()).unwrap();
            reader.index_source(file, &default_context()).unwrap();
        }
        assert_eq!(reader.work_stats().parsed_bytes, 0);
        let item = reader.parse_definition(&path("value")).unwrap().value;
        assert_eq!(reader.work_stats().parsed_bytes, source.len() as u64);
        assert!(reader.lower_to_tir(&path("value")).is_ok());
        assert!(reader.body_validation_runs(&path("value")) > 0);
        let file = reader
            .sources
            .add("heads.hir", source.replace("7u32", "8u32"))
            .unwrap();
        reader.index_source(file, &default_context()).unwrap();
        assert_ne!(
            reader.definition_items(&path("value")).unwrap()[0].text_hash,
            item.text_hash
        );
        assert_eq!(reader.work_stats().parsed_bytes, 2 * source.len() as u64);
        assert!(reader.parse_definition(&path("other")).is_ok());
        assert_eq!(
            reader.work_stats().parsed_bytes,
            (2 * source.len() + other.len()) as u64
        );
    });
}

#[test]
fn ordinary_parse_heads_preserve_duplicate_order_when_cached_and_new_files_mix() {
    with_compiler_stack(|| {
        let run = || {
            for reverse in [false, true] {
                let store = Arc::new(store::MemStore::default());
                let mut value = entry_path(&default_context());
                value.0.name = "value".into();
                let source = "fn value()->u32{7u32}\n";
                let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
                    .with_result_store(store.clone());
                let file = writer.sources.add("z.hir", source.into()).unwrap();
                writer.index_source(file, &default_context()).unwrap();
                drop(writer);
                // Exercise the same heads-only duplicate transaction as
                // before Parse outputs were independently persisted.
                let mut removed_syntax = 0;
                for (key, bytes) in store.take() {
                    if key.2 == QueryKind::Parse && bytes.starts_with(&2u32.to_le_bytes()) {
                        removed_syntax += 1;
                    } else {
                        store::ResultStore::put(&*store, &key, &bytes);
                    }
                }
                assert_eq!(removed_syntax, 1);
                let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
                    .with_result_store(store.clone());
                let names = if reverse {
                    ["z.hir", "a.hir"]
                } else {
                    ["a.hir", "z.hir"]
                };
                for name in names {
                    let file = reader.sources.add(name, source.into()).unwrap();
                    reader.index_source(file, &default_context()).unwrap();
                }
                // A newly parsed duplicate demands the cached candidate so the
                // existing declaration transaction assigns both ordinals.
                assert_eq!(
                    reader.work_stats().parsed_bytes,
                    if reverse {
                        2 * source.len() as u64
                    } else {
                        source.len() as u64
                    }
                );
                let output = reader.parse_definition(&value).unwrap();
                assert_eq!(output.value.occurrence, 0);
                let candidates = reader.definition_items(&value).unwrap();
                assert_eq!(candidates.len(), 2);
                let files: Vec<_> = candidates
                    .iter()
                    .map(|c| {
                        let (file, _) = reader.sources.anchor(c.anchor).unwrap();
                        (
                            reader.sources.file(file).unwrap().path.clone(),
                            c.occurrence,
                        )
                    })
                    .collect();
                assert_eq!(files, [("a.hir".into(), 0), ("z.hir".into(), 1)]);
                assert!(
                    reader.item_signature(&value).is_err(),
                    "duplicate declarations still reject"
                );
            }
            "ordered".to_owned()
        };
        if let Some(results) = hirsel_intern::under_salts(
            "ca_store_19::ordinary_parse_heads_preserve_duplicate_order_when_cached_and_new_files_mix",
            &[1, 9],
            run,
        ) {
            assert_eq!(results, ["ordered", "ordered"]);
        }
    });
}

#[test]
fn ordinary_summary_demands_restore_compact_facts_without_syntax() {
    with_compiler_stack(|| {
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(Observed::default());
            let source = "fn value(x:u32)->u32{x+1u32}\nfn caller()->u32{value(4u32)}\n";
            let context = default_context();
            let paths: Vec<_> = ["value", "caller"]
                .map(|name| {
                    let mut path = entry_path(&context);
                    path.0.name = name.into();
                    path
                })
                .into();
            let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = writer
                .sources
                .add("summary-heads.hir", source.into())
                .unwrap();
            writer.index_source(file, &context).unwrap();
            let outcomes = writer.publish_definitions(
                paths.clone(),
                hirsel_intern::jobs::Jobs::new(jobs),
                memo::Retention::Batch,
            );
            assert!(outcomes.iter().all(|o| o.result.is_ok()), "{outcomes:?}");
            let expected: Vec<_> = paths
                .iter()
                .map(|path| {
                    writer
                        .definition_summary_identity(hirsel_syntax::DefId::intern(path))
                        .unwrap()
                        .unwrap()
                })
                .collect();
            store.mode_reads.store(0, Ordering::Relaxed);
            let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            reader.adapters.parser = None;
            reader.adapters.lexer = None;
            reader.adapters.signature = None;
            reader.adapters.modes = None;
            reader.sources.add("padding.hir", "\n".into()).unwrap();
            let file = reader
                .sources
                .add("summary-heads.hir", source.into())
                .unwrap();
            reader.index_source(file, &context).unwrap();
            for (path, identity) in paths.iter().zip(expected) {
                let id = hirsel_syntax::DefId::intern(path);
                assert_eq!(
                    reader.definition_summary_identity(id).unwrap(),
                    Some(identity)
                );
                assert!(reader.definition_summary(id).unwrap().checked.is_some());
                assert_eq!(reader.recompute_count(path, QueryKind::Check), 0);
                assert_eq!(reader.recompute_count(path, QueryKind::Modes), 0);
                assert_eq!(reader.recompute_count(path, QueryKind::Summary), 0);
                assert!(reader.definition_items(path).is_err());
            }
            assert_eq!(reader.work_stats().parsed_bytes, 0);
            assert_eq!(store.mode_reads.load(Ordering::Relaxed), 0);
            // Recompute mode remains an independent oracle, not a cache admission.
            let mut oracle = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone())
                .with_verify_store(store::VerifyStore::Recompute);
            let file = oracle
                .sources
                .add("summary-heads.hir", source.into())
                .unwrap();
            oracle.index_source(file, &context).unwrap();
            let checked = oracle.publish_definitions(
                paths,
                hirsel_intern::jobs::Jobs::new(jobs),
                memo::Retention::Batch,
            );
            assert!(checked.iter().all(|o| o.result.is_ok()), "{checked:?}");
            assert!(oracle.work_stats().parsed_bytes > 0);
        }
    });
}

#[test]
fn ordinary_program_loading_reuses_source_heads_and_observes_changed_bytes() {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("hirsel-ordinary-loader-")
            .tempdir()
            .unwrap();
        let dir = scratch.path();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]},{"path":"helper.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let main = "pub fn main()->Result[(),()]{let _=helper();Ok(())}\n";
        let helper = "fn zebra()->u32{1u32}\nfn helper()->u32{zebra()}\nfn alpha()->u32{2u32}\n";
        std::fs::write(dir.join("main.hir"), main).unwrap();
        std::fs::write(dir.join("helper.hir"), helper).unwrap();
        for jobs in [1, 2, 4, 8] {
            let config = CompileConfig {
                store: Arc::new(store::MemStore::default()),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                ..CompileConfig::for_memory_cap(None)
            };
            let compiler = COMPILER.clone().with_config(config);
            let mut writer = compiler.load_program_source(&manifest, None).unwrap();
            assert_eq!(
                writer.engine.work_stats().parsed_bytes,
                (main.len() + helper.len()) as u64
            );
            let first = writer.check(LoweringPolicy::Reachable).unwrap();
            assert_eq!(first.completion, CheckCompletion::Complete);
            assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
            let mut reader = compiler.load_program_source(&manifest, None).unwrap();
            assert_eq!(reader.engine.work_stats().parsed_bytes, 0);
            reader.parse_sources().unwrap();
            assert_eq!(reader.engine.work_stats().parsed_bytes, 0);
            let id = hirsel_syntax::DefId::intern(&reader.entry);
            reader.engine.adapters.parser = None;
            reader.engine.adapters.lexer = None;
            reader.engine.adapters.modes = None;
            assert_eq!(
                reader.engine.definition_summary_identity(id).unwrap(),
                writer.engine.definition_summary_identity(id).unwrap()
            );
            assert_eq!(reader.engine.work_stats().parsed_bytes, 0);
            let mut checked = compiler.load_program_source(&manifest, None).unwrap();
            let output = checked.check(LoweringPolicy::Reachable).unwrap();
            assert_eq!(output.completion, first.completion);
            assert_eq!(output.diagnostics, first.diagnostics);
            // Equal length and same path do not hide changed source bytes.
            std::fs::write(dir.join("helper.hir"), helper.replacen("1u32", "?u32", 1)).unwrap();
            assert!(compiler.load_program_source(&manifest, None).is_err());
            std::fs::write(dir.join("helper.hir"), helper).unwrap();
        }
    });
}

#[test]
fn ordinary_program_check_restores_lint_and_completion_without_syntax() {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("hirsel-ordinary-check-")
            .tempdir()
            .unwrap();
        let dir = scratch.path();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let source = "type Failure = Used() | Stale()\nfn parse()->Result[u8,Failure]{Error(Used())}\npub fn main()->Result[(),()]{Ok(())}\n";
        std::fs::write(dir.join("main.hir"), source).unwrap();
        for jobs in [1, 2, 4, 8] {
            let compiler = COMPILER.clone().with_config(CompileConfig {
                store: Arc::new(store::MemStore::default()),
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                ..CompileConfig::for_memory_cap(None)
            });
            let mut writer = compiler.load_program_source(&manifest, None).unwrap();
            let expected = writer.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(expected.completion, CheckCompletion::Complete);
            assert_eq!(expected.diagnostics.len(), 1, "{:?}", expected.diagnostics);
            assert!(expected.diagnostics[0].message.contains("`Stale`"));
            let mut reader = compiler.load_program_source(&manifest, None).unwrap();
            reader.engine.adapters.parser = None;
            reader.engine.adapters.lexer = None;
            reader.engine.adapters.signature = None;
            let actual = reader.check(LoweringPolicy::Comptime).unwrap();
            assert_eq!(actual.completion, expected.completion);
            assert_eq!(
                actual
                    .diagnostics
                    .iter()
                    .map(|d| d.render_human(&actual.sources))
                    .collect::<Vec<_>>(),
                expected
                    .diagnostics
                    .iter()
                    .map(|d| d.render_human(&expected.sources))
                    .collect::<Vec<_>>()
            );
            assert_eq!(reader.engine.work_stats().parsed_bytes, 0);
            for name in ["main", "parse", "Failure"] {
                let mut path = reader.entry.clone();
                path.0.name = name.into();
                assert_eq!(reader.engine.recompute_count(&path, QueryKind::Modes), 0);
                assert!(reader.engine.definition_items(&path).is_err());
            }
        }
    });
}

#[test]
fn ordinary_source_heads_keep_growth_rejections_and_private_execution_authority() {
    with_compiler_stack(|| {
        let source = "fn first[T:copyable](x:T){second((copy x,copy x))}\nfn second[U:copyable](x:U){first(x)}\n";
        let store = Arc::new(store::MemStore::default());
        let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = writer
            .sources
            .add("growth-heads.hir", source.into())
            .unwrap();
        writer.index_source(file, &default_context()).unwrap();
        let outcomes = writer.publish_definitions(
            vec![definition("first"), definition("second")],
            hirsel_intern::jobs::Jobs::new(2),
            memo::Retention::Batch,
        );
        assert!(outcomes.iter().all(|o| o.result.is_ok()));
        let expected = writer.generic_graph_diagnostics().unwrap();
        assert_eq!(expected.len(), 1);
        assert_eq!(expected[0].rule.as_str(), "GEN-5");
        let mut reader =
            QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(store);
        let file = reader
            .sources
            .add("growth-heads.hir", source.into())
            .unwrap();
        reader.index_source(file, &default_context()).unwrap();
        reader.adapters.parser = None;
        reader.adapters.modes = None;
        for name in ["first", "second"] {
            reader
                .definition_summary(hirsel_syntax::DefId::intern(&definition(name)))
                .unwrap();
        }
        let actual = reader.generic_graph_diagnostics().unwrap();
        assert_eq!(
            actual
                .iter()
                .map(|d| d.render_human(&reader.sources))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|d| d.render_human(&writer.sources))
                .collect::<Vec<_>>()
        );
        assert_eq!(reader.work_stats().parsed_bytes, 0);

        let source = "fn helper()->u32{7u32}\nconst VALUE:u32=comptime helper()\n";
        let owner = definition("VALUE");
        for verify in [Verify::On, Verify::Off] {
            let store = Arc::new(store::MemStore::default());
            let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            writer.verify = verify;
            let file = writer
                .sources
                .add("private-heads.hir", source.into())
                .unwrap();
            writer.index_source(file, &default_context()).unwrap();
            writer.lower_to_tir(&owner).unwrap();
            let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store);
            let file = reader
                .sources
                .add("private-heads.hir", source.into())
                .unwrap();
            reader.index_source(file, &default_context()).unwrap();
            reader.adapters.lexer = None;
            reader.adapters.parser = None;
            reader.adapters.signature = None;
            reader.adapters.modes = None;
            reader.adapters.lowering = None;
            assert_eq!(
                reader.lower_to_tir(&owner).is_ok(),
                verify == Verify::On,
                "syntax-free metadata supplies no private completion: {verify:?}"
            );
            assert_eq!(reader.work_stats().parsed_bytes, 0);
        }
    });
}

#[test]
fn ordinary_source_audits_validate_lexical_heads_without_syntax_and_keep_local_authority() {
    with_compiler_stack(|| {
        let source = "fn helper()->u32{7u32}\nfn plain(x:u32)->u32{x+1u32}\nfn staged()->u32{comptime helper()}\n";
        let store = Arc::new(store::MemStore::default());
        let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = writer
            .sources
            .add("audit-heads.hir", source.into())
            .unwrap();
        writer.index_source(file, &default_context()).unwrap();
        let plain = writer.source_audit(&definition("plain")).unwrap();
        let staged = writer.source_audit(&definition("staged")).unwrap();
        let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        reader.sources.add("padding.hir", "\n".into()).unwrap();
        let file = reader
            .sources
            .add("audit-heads.hir", source.into())
            .unwrap();
        reader.index_source(file, &default_context()).unwrap();
        reader.adapters.lexer = None;
        reader.adapters.parser = None;
        reader.adapters.signature = None;
        reader.adapters.modes = None;
        reader.adapters.lowering = None;
        assert_eq!(reader.source_audit(&definition("plain")).unwrap(), plain);
        assert_eq!(reader.source_audit(&definition("staged")).unwrap(), staged);
        assert_eq!(reader.work_stats().parsed_bytes, 0);
        for name in ["plain", "staged"] {
            assert_eq!(
                reader.recompute_count(&definition(name), QueryKind::Modes),
                0
            );
            assert_eq!(reader.lower_adapter_calls(&definition(name)), 0);
        }
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut untrusted =
            QueryEngine::new(default_environment(), compiler_adapters()).with_result_store(copied);
        let file = untrusted
            .sources
            .add("audit-heads.hir", source.into())
            .unwrap();
        untrusted.index_source(file, &default_context()).unwrap();
        untrusted.adapters.lexer = None;
        untrusted.adapters.parser = None;
        untrusted.adapters.modes = None;
        untrusted.adapters.lowering = None;
        assert!(
            untrusted.source_audit(&definition("staged")).is_err(),
            "ordinary syntax-free heads do not transfer local V completion"
        );
        let mut oracle = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store)
            .with_verify_store(store::VerifyStore::Recompute);
        let file = oracle
            .sources
            .add("audit-heads.hir", source.into())
            .unwrap();
        oracle.index_source(file, &default_context()).unwrap();
        assert_eq!(oracle.source_audit(&definition("plain")).unwrap(), plain);
        assert_eq!(oracle.source_audit(&definition("staged")).unwrap(), staged);
    });
}

#[test]
fn ordinary_staged_registrations_restore_from_heads_without_syntax_or_execution_authority() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "fn value[n:size]()->u64{comptime (n*n)}\n";
        let owner = definition("value");
        let store = Arc::new(store::MemStore::default());
        let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = writer
            .sources
            .add("staged-heads.hir", source.into())
            .unwrap();
        writer.index_source(file, &default_context()).unwrap();
        let lowered = writer.lower_to_tir(&owner).unwrap();
        let hidden = lowered
            .value
            .bodies
            .iter()
            .flat_map(|b| &b.rvalues)
            .find_map(|v| match v {
                hirsel_tir::Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        for n in [2, 3] {
            writer.constant_instance(&hidden, &[n]).unwrap();
        }
        let open = |store: Arc<dyn store::ResultStore>| {
            let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store);
            reader.sources.add("padding.hir", "\n".into()).unwrap();
            let file = reader
                .sources
                .add("staged-heads.hir", source.into())
                .unwrap();
            reader.index_source(file, &default_context()).unwrap();
            reader.adapters.lexer = None;
            reader.adapters.parser = None;
            reader.adapters.signature = None;
            reader.adapters.modes = None;
            reader.adapters.lowering = None;
            reader
        };
        let mut reader = open(store.clone());
        reader.register_staged_definition(&owner).unwrap();
        reader.lower_to_tir(&owner).unwrap();
        for n in [2, 3] {
            assert_eq!(
                reader.constant_instance(&hidden, &[n]).unwrap().root.kind,
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(u128::from(n * n)))
            );
        }
        assert!(reader.constant_instance(&hidden, &[4]).is_err());
        assert!(reader.constant_instance(&hidden, &[2, 3]).is_err());
        assert_eq!(reader.work_stats().parsed_bytes, 0);
        assert_eq!(reader.recompute_count(&owner, QueryKind::Modes), 0);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut untrusted = open(copied);
        untrusted.register_staged_definition(&owner).unwrap();
        assert!(
            untrusted.constant_instance(&hidden, &[2]).is_err(),
            "ordinary registration carries no execution completion"
        );
        let mut oracle = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store)
            .with_verify_store(store::VerifyStore::Recompute);
        let file = oracle
            .sources
            .add("staged-heads.hir", source.into())
            .unwrap();
        oracle.index_source(file, &default_context()).unwrap();
        oracle.register_staged_definition(&owner).unwrap();
        oracle.lower_to_tir(&owner).unwrap();
        assert_eq!(
            oracle.constant_instance(&hidden, &[2]).unwrap().root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4))
        );
    });
}

#[test]
fn ordinary_constant_values_reuse_heads_without_syntax_and_refuse_unverified_completion() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let source = "fn helper()->u32{7u32}\nconst VALUE:u32=comptime helper()\n";
        let owner = definition("VALUE");
        for verify in [Verify::On, Verify::Off] {
            let store = Arc::new(store::MemStore::default());
            let mut writer = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            writer.verify = verify;
            let file = writer
                .sources
                .add("value-heads.hir", source.into())
                .unwrap();
            writer.index_source(file, &default_context()).unwrap();
            assert_eq!(
                writer.materialize_constant(&owner).unwrap(),
                hirsel_tir::Constant::Integer(7)
            );
            let open = |store: Arc<dyn store::ResultStore>| {
                let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
                    .with_result_store(store);
                reader.sources.add("padding.hir", "\n".into()).unwrap();
                let file = reader
                    .sources
                    .add("value-heads.hir", source.into())
                    .unwrap();
                reader.index_source(file, &default_context()).unwrap();
                reader.adapters.lexer = None;
                reader.adapters.parser = None;
                reader.adapters.signature = None;
                reader.adapters.modes = None;
                reader.adapters.lowering = None;
                reader
            };
            let mut reader = open(store.clone());
            let actual = reader.materialize_constant(&owner);
            if verify == Verify::On {
                assert_eq!(actual.unwrap(), hirsel_tir::Constant::Integer(7));
                assert!(reader.constant_definitions().unwrap().contains(&owner));
            } else {
                assert!(actual.is_err(), "heads cannot mint private V completion");
            }
            assert_eq!(reader.work_stats().parsed_bytes, 0);
            assert_eq!(reader.recompute_count(&owner, QueryKind::Modes), 0);
            let copied = Arc::new(store::MemStore::default());
            for (key, bytes) in store.take() {
                store::ResultStore::put(&*copied, &key, &bytes);
                store::ResultStore::put(&*store, &key, &bytes);
            }
            assert!(
                open(copied).materialize_constant(&owner).is_err(),
                "ordinary copied values have no verifier authority"
            );
            if verify == Verify::On {
                let mut oracle = QueryEngine::new(default_environment(), compiler_adapters())
                    .with_result_store(store)
                    .with_verify_store(store::VerifyStore::Recompute);
                let file = oracle
                    .sources
                    .add("value-heads.hir", source.into())
                    .unwrap();
                oracle.index_source(file, &default_context()).unwrap();
                assert_eq!(
                    oracle.materialize_constant(&owner).unwrap(),
                    hirsel_tir::Constant::Integer(7)
                );
            }
        }
    });
}

#[test]
fn package_precomputation_publishes_only_ordinary_products_and_keeps_verifier_authority_local() {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("hirsel-precompute-")
            .tempdir()
            .unwrap();
        let dir = scratch.path();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"api.hir","module":[]}],"dependencies":[]}).to_string()).unwrap();
        std::fs::write(
            dir.join("api.hir"),
            "pub fn value()->u32{7u32}\npub const VALUE:u32=comptime value()\n",
        )
        .unwrap();
        let bundle = dir.join("ordinary-products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        compiler.precompute_package(&manifest, &bundle).unwrap();
        let local = dir.join("local-products");
        let compiler = compiler.with_config(CompileConfig {
            store: Arc::new(
                store::DirStore::new(local.clone()).with_read_only_root(bundle.clone()),
            ),
            ..CompileConfig::for_memory_cap(None)
        });
        let mut reader = compiler.load_check_package(&manifest).unwrap();
        assert_eq!(reader.engine.work_stats().parsed_bytes, 0);
        reader.engine.adapters.lexer = None;
        reader.engine.adapters.parser = None;
        reader.engine.adapters.signature = None;
        reader.engine.adapters.modes = None;
        reader.engine.adapters.lowering = None;
        let mut value = reader.entry.clone();
        value.0.name = "value".into();
        assert!(
            reader
                .engine
                .definition_summary(hirsel_syntax::DefId::intern(&value))
                .is_ok()
        );
        assert!(reader.engine.semantic_program_revision(&value).is_ok());
        let mut constant = value.clone();
        constant.0.name = "VALUE".into();
        assert!(
            reader.engine.materialize_constant(&constant).is_err(),
            "packaged producer products confer no private execution authority"
        );
        assert_eq!(reader.engine.work_stats().parsed_bytes, 0);
        assert!(!local.join("hirsel-std.image").exists());
        let mut fresh = compiler.load_check_package(&manifest).unwrap();
        assert_eq!(
            fresh.engine.materialize_constant(&constant).unwrap(),
            hirsel_tir::Constant::Integer(7)
        );
        // After first local V, reopening the same writable authority reuses it.
        let mut warm = compiler.load_check_package(&manifest).unwrap();
        warm.engine.adapters.lexer = None;
        warm.engine.adapters.parser = None;
        warm.engine.adapters.signature = None;
        warm.engine.adapters.modes = None;
        warm.engine.adapters.lowering = None;
        assert_eq!(
            warm.engine.materialize_constant(&constant).unwrap(),
            hirsel_tir::Constant::Integer(7)
        );
        assert_eq!(warm.engine.work_stats().parsed_bytes, 0);
    });
}

#[test]
fn ordinary_precomputed_dependency_products_do_not_recheck_bodies_during_native_export() {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("hirsel-precompute-client-")
            .tempdir()
            .unwrap();
        let dir = scratch.path();
        let library = dir.join("library.json");
        std::fs::write(&library, serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"api.hir","module":["lib"]}],"dependencies":[]}).to_string()).unwrap();
        std::fs::write(dir.join("api.hir"), "pub fn value()->u32{7u32}\npub const VALUE:u32=comptime value()\npub fn identity[T](take x:T)->T{x}\npub type Plain { value:u32 }\n").unwrap();
        let manifest = dir.join("client.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"aaa","version":"1"},"sources":[{"path":"client.hir","module":[]}],"dependencies":[{"manifest":"library.json","package":{"name":"library","version":"1"},"expose":[{"head":"lib","module":["lib"]}]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        std::fs::write(
            dir.join("client.hir"),
            "pub fn main()->Result[(),()]{let _=lib.identity(lib.value());Ok(())}\n",
        )
        .unwrap();
        let products = dir.join("products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        compiler.precompute_package(&library, &products).unwrap();
        let shared = compiler.with_config(CompileConfig {
            store: Arc::new(store::DirStore::new(products)),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        let mut cached = shared.load_program_source(&manifest, None).unwrap();
        let paths: Vec<_> = cached
            .engine
            .indexed_definitions()
            .into_iter()
            .filter(|path| path.0.package.name == "library")
            .collect();
        cached.check_for_execution().unwrap();
        for path in &paths {
            cached.engine.semantic_program_revision(path).unwrap();
            if cached.engine.lower_to_tir(path).is_ok() {
                cached.engine.source_audit(path).unwrap();
            }
        }
        cached.engine.export_native_input().unwrap();
        for path in &paths {
            assert_eq!(
                cached.engine.recompute_count(path, QueryKind::Check),
                0,
                "{path}"
            );
            assert_eq!(
                cached.engine.recompute_count(path, QueryKind::Modes),
                0,
                "{path}"
            );
        }
    });
}

#[test]
fn ordinary_actor_generic_products_do_not_recheck_bodies_during_native_export() {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("hirsel-precompute-client-")
            .tempdir()
            .unwrap();
        let dir = scratch.path();
        let library = dir.join("library.json");
        std::fs::write(&library, serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"api.hir","module":["lib"]}],"dependencies":[]}).to_string()).unwrap();
        std::fs::write(
            dir.join("api.hir"),
            std::fs::read_to_string(
                crate::image_fixture::std_manifest()
                    .parent()
                    .unwrap()
                    .join("actor.hir"),
            )
            .unwrap(),
        )
        .unwrap();
        let manifest = dir.join("client.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"aaa","version":"1"},"sources":[{"path":"client.hir","module":[]}],"dependencies":[{"manifest":"library.json","package":{"name":"library","version":"1"},"expose":[{"head":"lib","module":["lib"]}]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        std::fs::write(
            dir.join("client.hir"),
            "pub fn main()->Result[(),()]{Ok(())}\n",
        )
        .unwrap();
        let products = dir.join("products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        compiler.precompute_package(&library, &products).unwrap();
        let shared = compiler.with_config(CompileConfig {
            store: Arc::new(store::DirStore::new(products)),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        let mut cached = shared.load_program_source(&manifest, None).unwrap();
        let paths: Vec<_> = cached
            .engine
            .indexed_definitions()
            .into_iter()
            .filter(|path| path.0.package.name == "library")
            .collect();
        cached.check_for_execution().unwrap();
        for path in &paths {
            cached.engine.semantic_program_revision(path).unwrap();
            if cached.engine.lower_to_tir(path).is_ok() {
                cached.engine.source_audit(path).unwrap();
            }
        }
        cached.engine.export_native_input().unwrap();
        for path in &paths {
            assert_eq!(
                cached.engine.recompute_count(path, QueryKind::Check),
                0,
                "{path}"
            );
            assert_eq!(
                cached.engine.recompute_count(path, QueryKind::Modes),
                0,
                "{path}"
            );
        }
    });
}

#[test]
fn ordinary_read_only_constant_products_replay_without_rechecking() {
    read_only_constant(false);
}

#[test]
fn ordinary_read_only_constant_modes_are_available_before_execution() {
    read_only_constant(true);
}

fn read_only_constant(prefetch: bool) {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("hirsel-readonly-constant-")
            .tempdir()
            .unwrap();
        let dir = scratch.path();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"api.hir","module":[]}],"dependencies":[]}).to_string()).unwrap();
        std::fs::write(dir.join("api.hir"), "pub const MOD:u32=65521u32\n").unwrap();
        let bundle = dir.join("ordinary-products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        compiler.precompute_package(&manifest, &bundle).unwrap();
        let compiler = compiler.with_config(CompileConfig {
            store: Arc::new(store::DirStore::new(dir.join("local")).with_read_only_root(bundle)),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        let mut reader = compiler.load_check_package(&manifest).unwrap();
        let mut constant = reader.entry.clone();
        constant.0.name = "MOD".into();
        if prefetch {
            reader.engine.mode_definition(&constant).unwrap();
        }
        assert_eq!(
            reader.engine.materialize_constant(&constant).unwrap(),
            hirsel_tir::Constant::Integer(65521)
        );
        for kind in [QueryKind::Check, QueryKind::Modes] {
            assert_eq!(
                reader.engine.recompute_count(&constant, kind),
                0,
                "{constant}: {kind:?}"
            );
        }
    });
}

#[test]
fn ordinary_read_only_executed_helper_keeps_its_check_product() {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"api.hir","module":[]}],"dependencies":[]}).to_string()).unwrap();
        std::fs::write(dir.join("api.hir"), "fn alphabet_bytes(x:u32)->u32{x+1u32}\nfn decode_words()->u32{alphabet_bytes(6u32)}\npub const DECODE:u32=comptime decode_words()\n").unwrap();
        let products = dir.join("products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        compiler.precompute_package(&manifest, &products).unwrap();
        let mut reader = compiler
            .with_config(CompileConfig {
                store: Arc::new(
                    store::DirStore::new(dir.join("reader")).with_read_only_root(products),
                ),
                jobs: hirsel_intern::jobs::Jobs::new(1),
                ..CompileConfig::for_memory_cap(None)
            })
            .load_check_package(&manifest)
            .unwrap();
        let mut root = reader.entry.clone();
        root.0.name = "DECODE".into();
        assert_eq!(
            reader.engine.materialize_constant(&root).unwrap(),
            hirsel_tir::Constant::Integer(7)
        );
        for name in ["alphabet_bytes", "decode_words", "DECODE"] {
            root.0.name = name.into();
            reader.engine.semantic_program_revision(&root).unwrap();
            reader.engine.lower_to_tir(&root).unwrap();
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    reader.engine.recompute_count(&root, phase),
                    0,
                    "{root}: {phase:?}"
                );
            }
        }
    });
}

#[test]
fn ordinary_generated_package_reopens_without_signature_or_constructor_errors() {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path();
        std::fs::write(dir.join("model.hir"),"pub type Draft { title:String }\nfn generator()->Result[Body[fn() -> u32 uses {}],String]{Ok(codegen.body(\"fn() -> u32 uses {} { 1u32 }\"))}\npub fn generated()->u32 uses {}=comptime generator()\n").unwrap();
        std::fs::write(
            dir.join("client.hir"),
            "fn make()->Draft{Draft(title:\"x\")}\n",
        )
        .unwrap();
        let manifest = dir.join("hirsel.json");
        std::fs::write(&manifest,serde_json::json!({"schema":1,"package":{"name":"library","version":"1"},"sources":[{"path":"model.hir","module":["model"]},{"path":"client.hir","module":["model"]}],"dependencies":[]}).to_string()).unwrap();
        let compiler = COMPILER.clone().with_config(CompileConfig {
            store: Arc::new(store::DirStore::new(dir.join("products"))),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        for pass in ["cold", "warm"] {
            let mut reader = compiler.load_check_package(&manifest).unwrap();
            let output = reader
                .check(LoweringPolicy::Reachable)
                .unwrap_or_else(|e| panic!("{pass}: {e:?}"));
            assert_eq!(output.completion, CheckCompletion::Complete, "{pass}");
            assert!(
                output.diagnostics.is_empty(),
                "{pass}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn heads_only_signature_reuse_preserves_the_complete_durable_record() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let context = default_context();
        let source = "const LEFT:Inline[u16,2]=[1,2]\n";
        let left = definition("LEFT");
        let mut seed = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = seed.sources.add("left.hir", source.into()).unwrap();
        seed.index_source(file, &context).unwrap();
        seed.item_signature(&left).unwrap();
        assert!(
            seed.resolver_snapshot(&left, QueryKind::Signature)
                .is_some()
        );
        let mut entries = store.take();
        let original: Vec<_> = entries
            .iter()
            .filter(|(key, _)| key.2 == QueryKind::Signature)
            .map(|(key, bytes)| (key.clone(), bytes.clone()))
            .collect();
        assert_eq!(original.len(), 1);
        for (key, bytes) in entries.drain() {
            store::ResultStore::put(&*store, &key, &bytes);
        }
        drop(seed);
        let mut reader = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = reader.sources.add("moved/left.hir", source.into()).unwrap();
        reader.index_source(file, &context).unwrap();
        assert_eq!(reader.work_stats().parsed_bytes, 0);
        reader.item_signature(&left).unwrap();
        assert!(
            reader
                .resolver_snapshot(&left, QueryKind::Signature)
                .is_none()
        );
        let client = reader
            .sources
            .add("client.hir", "const NEXT:u16=comptime LEFT[1]\n".into())
            .unwrap();
        reader.index_source(client, &context).unwrap();
        assert_eq!(
            reader.materialize_constant(&definition("NEXT")).unwrap(),
            hirsel_tir::Constant::Integer(2)
        );
        for (key, bytes) in &original {
            assert_eq!(
                &*store::ResultStore::get(&*store, key).unwrap(),
                &**bytes,
                "a heads-only in-memory projection must not replace its full durable signature"
            );
        }
        let mut oracle = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store)
            .with_verify_store(store::VerifyStore::Recompute);
        let file = oracle.sources.add("left.hir", source.into()).unwrap();
        oracle.index_source(file, &context).unwrap();
        oracle.item_signature(&left).unwrap();
    });
}

#[test]
fn frozen_workers_reuse_locally_validated_constant_inputs_without_rechecking() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source = "const BASE:u8=1u8\nconst NEXT:u8=comptime BASE+1u8\n";
        let base = definition("BASE");
        let next = definition("NEXT");
        let mut seed = signature_session(store.clone(), source, true);
        assert_eq!(
            seed.materialize_constant(&base).unwrap(),
            hirsel_tir::Constant::Integer(1)
        );
        drop(seed);
        let mut reader = signature_session(store.clone(), source, true);
        assert_eq!(
            reader.materialize_constant(&base).unwrap(),
            hirsel_tir::Constant::Integer(1)
        );
        assert_eq!(reader.recompute_count(&base, QueryKind::Check), 0);
        let result = reader
            .materialize_value_outcomes(vec![next.clone()], hirsel_intern::jobs::Jobs::new(1));
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].result.as_ref().unwrap().root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(2))
        );
        assert_eq!(
            reader.recompute_count(&base, QueryKind::Check),
            0,
            "a frozen nested demand must reuse the controller's locally validated constant"
        );
        let mut changed =
            signature_session(store, &source.replace("BASE:u8=1u8", "BASE:u8=2u8"), true);
        let result =
            changed.materialize_value_outcomes(vec![next], hirsel_intern::jobs::Jobs::new(1));
        assert_eq!(
            result[0].result.as_ref().unwrap().root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(3))
        );
    });
}

#[test]
fn frozen_nested_body_demand_restores_ordinary_modes_before_rechecking() {
    with_compiler_stack(|| {
        let store = Arc::new(store::MemStore::default());
        let source = "fn helper(x:u8)->u8{x+1u8}\nconst NEXT:u8=comptime helper(1u8)\n";
        let helper = definition("helper");
        let next = definition("NEXT");
        let mut seed = signature_session(store.clone(), source, true);
        seed.mode_definition(&helper).unwrap();
        drop(seed);
        let mut reader = signature_session(store.clone(), source, true);
        let result = reader
            .materialize_value_outcomes(vec![next.clone()], hirsel_intern::jobs::Jobs::new(1));
        assert_eq!(
            result[0].result.as_ref().unwrap().root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(2))
        );
        assert_eq!(
            reader.recompute_count(&helper, QueryKind::Check),
            0,
            "nested execution must try its persisted Modes before running P again"
        );
        let mut edited = signature_session(store, &source.replace("x+1u8", "x+2u8"), true);
        let result =
            edited.materialize_value_outcomes(vec![next], hirsel_intern::jobs::Jobs::new(1));
        assert_eq!(
            result[0].result.as_ref().unwrap().root.kind,
            hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(3))
        );
        assert!(edited.recompute_count(&helper, QueryKind::Check) > 0);
    });
}
