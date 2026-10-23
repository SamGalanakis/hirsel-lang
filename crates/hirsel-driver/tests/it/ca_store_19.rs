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
        let declarations = lowered.value.bodies.iter().flat_map(|body| &body.rvalues)
            .filter_map(|value| match value {
                hirsel_tir::Rvalue::ReadConstant { definition, arguments } if !arguments.is_empty() => {
                    let signature = snapshot.signature(hirsel_syntax::DefId::intern(definition)).unwrap().unwrap();
                    Some((definition.clone(), signature.fingerprint))
                }
                _ => None,
            }).collect::<Vec<_>>();
        assert_eq!(declarations.len(), 1);
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.register_staged_definition(&path).unwrap();
        assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);
        for (path, fingerprint) in &declarations {
            assert_eq!(fresh.item_signature(path).unwrap().value.fingerprint, *fingerprint);
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
        fn name(&self) -> &'static str { "producer-only" }
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
        assert_eq!(local.body_validation_runs(&path), 0, "local verifier receipt");
        let mut remote = signature_session(Arc::new(ProducerOnly(store.clone())), source, false);
        remote.adapters.modes = None;
        remote.lower_to_tir(&path).unwrap();
        assert_eq!(remote.recompute_count(&path, QueryKind::Modes), 0);
        assert_eq!(remote.body_validation_runs(&path), 1, "producer bytes require first V");
        remote.lower_to_tir(&path).unwrap();
        assert_eq!(remote.body_validation_runs(&path), 1, "this process now owns exact evidence");
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
    let run = || with_compiler_stack(|| {
        use hirsel_intern::timings::{self, Count};
        timings::enable();
        let runs = || timings::report().counts.into_iter().find(|(kind, _)| *kind == Count::VRuns).unwrap().1;
        let source = "const A:u8=comptime 7u8\nconst B:u8=comptime 8u8\n";
        let paths = vec![definition("A"), definition("B")];
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            let before = runs();
            let expected = seed.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(expected.iter().all(Result::is_ok));
            assert_eq!(runs() - before, 2, "first private V at jobs={jobs}");
            let mut fresh = signature_session(store.clone(), source, true);
            let before = runs();
            let actual = fresh.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(actual.iter().all(Result::is_ok));
            assert_eq!(actual.iter().map(|v| &v.as_ref().unwrap().root.kind).collect::<Vec<_>>(), expected.iter().map(|v| &v.as_ref().unwrap().root.kind).collect::<Vec<_>>());
            assert_eq!(runs(), before, "verifier-owned private receipts at jobs={jobs}");
            let changed = source.replace("7u8", "9u8");
            let mut edited = signature_session(store, &changed, true);
            let before = runs();
            assert_eq!(edited.materialize_constant(&definition("A")).unwrap(), hirsel_tir::Constant::Integer(9));
            assert_eq!(runs() - before, 1, "changed private artifact requires V");
        }
        "private-receipts".to_owned()
    });
    if let Some(outputs) = hirsel_intern::under_salts(
        "ca_store_19::private_evaluation_uses_independent_receipts_in_scalar_and_worker_sessions", &[1], run,
    ) { assert_eq!(outputs, ["private-receipts"]); }
}

#[test]
fn scalar_and_staged_values_from_verify_off_require_private_admission() {
    with_compiler_stack(|| {
        let mut scalar = signature_session(Arc::new(store::MemStore::default()), "const A:u8=comptime 7u8\n", true);
        scalar.verify = Verify::Off;
        assert_eq!(scalar.materialize_constant(&definition("A")).unwrap(), hirsel_tir::Constant::Integer(7));
        scalar.verify = Verify::On;
        scalar.adapters.lowering = None;
        assert!(scalar.materialize_constant(&definition("A")).is_err(), "an unverified private value must demand its initializer");

        let mut staged = signature_session(Arc::new(store::MemStore::default()), "fn value[n:size]()->u64{comptime (n*n)}\n", true);
        staged.verify = Verify::Off;
        let lowered = staged.lower_to_tir(&definition("value")).unwrap();
        let hidden = lowered.value.bodies.iter().flat_map(|b| &b.rvalues).find_map(|v| match v {
            hirsel_tir::Rvalue::ReadConstant { definition, arguments } if !arguments.is_empty() => Some(definition.clone()),
            _ => None,
        }).unwrap();
        hirsel_tir::TirProvider::constant_instance(&mut staged, &hidden, &[2]).unwrap();
        staged.verify = Verify::On;
        staged.adapters.lowering = None;
        assert!(hirsel_tir::TirProvider::constant_instance(&mut staged, &hidden, &[2]).is_err(), "a staged VerifyOff value must demand private admission");
    });
}


#[test]
fn scalar_evaluation_products_require_local_completion_and_skip_analyzed_bodies() {
    with_compiler_stack(|| {
        let source = "fn helper()->u32{7u32}\nconst VALUE:u32=comptime helper()\n";
        let path = definition("VALUE");
        let store = Arc::new(store::MemStore::default());
        let mut seed = signature_session(store.clone(), source, true);
        for name in ["helper", "VALUE"] { seed.item_signature(&definition(name)).unwrap(); }
        assert_eq!(seed.materialize_constant(&path).unwrap(), hirsel_tir::Constant::Integer(7));
        drop(seed);
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        assert_eq!(fresh.materialize_constant(&path).unwrap(), hirsel_tir::Constant::Integer(7));
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
        assert!(untrusted.materialize_constant(&path).is_err(), "copied values cannot supply private V completion");
        let mut changed = signature_session(store, &source.replace("7u32", "9u32"), true);
        assert_eq!(changed.materialize_constant(&path).unwrap(), hirsel_tir::Constant::Integer(9));
    });
}

#[test]
fn worker_evaluation_products_adopt_only_local_complete_verdicts() {
    with_compiler_stack(|| {
        let source = "fn helper()->u32{7u32}\nconst A:u32=comptime helper()\nconst B:u32=comptime A+1u32\n";
        let paths = vec![definition("A"), definition("B")];
        for jobs in [1, 2, 4, 8] {
            let store = Arc::new(store::MemStore::default());
            let mut seed = signature_session(store.clone(), source, true);
            for name in ["helper", "A", "B"] { seed.item_signature(&definition(name)).unwrap(); }
            let expected = seed.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(expected.iter().all(Result::is_ok), "{expected:?}");
            let mut fresh = signature_session(store.clone(), source, false);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            let actual = fresh.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(actual.iter().all(Result::is_ok), "jobs={jobs}: {actual:?}");
            assert_eq!(actual.iter().map(|v| &v.as_ref().unwrap().root.kind).collect::<Vec<_>>(), expected.iter().map(|v| &v.as_ref().unwrap().root.kind).collect::<Vec<_>>());
            for path in &paths { assert_eq!(fresh.recompute_count(path, QueryKind::Modes), 0); }
            let copied = Arc::new(store::MemStore::default());
            for (key, bytes) in store.take() {
                store::ResultStore::put(&*copied, &key, &bytes);
                store::ResultStore::put(&*store, &key, &bytes);
            }
            let mut untrusted = signature_session(copied, source, false);
            untrusted.adapters.modes = None;
            untrusted.adapters.lowering = None;
            assert!(untrusted.materialize_values(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs)).iter().all(Result::is_err));
            let mut edited = signature_session(store, &source.replace("7u32", "9u32"), true);
            assert_eq!(edited.materialize_constant(&definition("B")).unwrap(), hirsel_tir::Constant::Integer(10));
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
            assert!(remote.lower_to_tir(&path).is_err(), "producer Lower bytes cannot supply private execution authority");
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
            assert!(on.lower_to_tir(&path).is_err(), "VerifyOff cannot mint completion");
        }
    });
}

#[test]
fn resident_lower_completion_rechecks_its_private_validation_context() {
    with_compiler_stack(|| {
        let source = "pub opaque type Hidden:plain uses {}{field:u32}\nfn value()->u32{comptime 7u32}\n";
        let store = Arc::new(store::MemStore::default());
        let mut engine = signature_session(store, source, true);
        let path = definition("value");
        engine.lower_to_tir(&path).unwrap();
        let before = engine.lower_adapter_calls(&path);
        engine.lower_to_tir(&path).unwrap();
        assert_eq!(engine.lower_adapter_calls(&path), before);
        let file = engine.sources.add("signatures.hir", source.replace("field:u32", "field:u64")).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        engine.lower_to_tir(&path).unwrap();
        assert_eq!(engine.lower_adapter_calls(&path), before + 1, "complete private V context changed");
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
        let hidden = lowered.value.bodies.iter().flat_map(|b| &b.rvalues).find_map(|v| match v {
            hirsel_tir::Rvalue::ReadConstant { definition, arguments } if !arguments.is_empty() => Some(definition.clone()),
            _ => None,
        }).unwrap();
        let expected: Vec<_> = [2, 3].into_iter().map(|n| seed.constant_instance(&hidden, &[n]).unwrap()).collect();
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        fresh.register_staged_definition(&owner).unwrap();
        for (n, expected) in [2, 3].into_iter().zip(&expected) {
            let actual = fresh.constant_instance(&hidden, &[n]).unwrap();
            assert_eq!(actual.root.kind, expected.root.kind, "selected size={n}");
        }
        assert_eq!(fresh.recompute_count(&owner, QueryKind::Modes), 0);
        assert!(fresh.constant_instance(&hidden, &[4]).is_err(), "unpublished instance cannot alias a selected size");
        assert!(fresh.constant_instance(&hidden, &[2, 3]).is_err(), "arity remains checked");

        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None;
        remote.adapters.lowering = None;
        remote.register_staged_definition(&owner).unwrap();
        assert!(remote.constant_instance(&hidden, &[2]).is_err(), "ordinary staged values cannot transfer private completion");

        let edited = source.replace("n*n", "n+n");
        let mut changed = signature_session(store.clone(), &edited, true);
        changed.register_staged_definition(&owner).unwrap();
        changed.adapters.modes = None;
        changed.adapters.lowering = None;
        assert!(changed.constant_instance(&hidden, &[2]).is_err(), "owner body edits invalidate all selected instances");

        let off_store = Arc::new(store::MemStore::default());
        let mut off = signature_session(off_store.clone(), source, true);
        off.verify = Verify::Off;
        off.register_staged_definition(&owner).unwrap();
        off.constant_instance(&hidden, &[2]).unwrap();
        let mut on = signature_session(off_store, source, true);
        on.register_staged_definition(&owner).unwrap();
        on.adapters.modes = None;
        on.adapters.lowering = None;
        assert!(on.constant_instance(&hidden, &[2]).is_err(), "VerifyOff staged products cannot mint private completion");
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
            let declarations = lowered.value.bodies[lowered.value.entry.0 as usize].rvalues.iter().filter_map(|v| match v {
                hirsel_tir::Rvalue::ReadConstant { definition, arguments } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            }).collect::<Vec<_>>();
            assert_eq!(declarations.len(), 1, "one explicit staged expression");
            let hidden = declarations[0].clone();
            let outcomes = seed.check_lower_definition_prefix(vec![definition("FIRST"), definition("BAD"), definition("LATER")], hirsel_intern::jobs::Jobs::new(jobs));
            assert_eq!(outcomes.len(), 2, "jobs={jobs}");
            assert!(outcomes[0].result.is_ok());
            assert!(outcomes[1].result.is_err());
            let mut fresh = signature_session(store.clone(), source, true);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            fresh.register_staged_definition(&definition("scaled")).unwrap();
            assert_eq!(fresh.constant_instance(&hidden, &[2]).unwrap().root.kind, hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)), "jobs={jobs}");
            assert!(fresh.constant_instance(&hidden, &[3]).is_err(), "later speculative size cannot publish at jobs={jobs}");
            let mut oracle = signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
            oracle.register_staged_definition(&definition("scaled")).unwrap();
            assert_eq!(oracle.constant_instance(&hidden, &[2]).unwrap().root.kind, hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)));
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
        let declaration = expected.value.bodies.iter().flat_map(|body| &body.rvalues).find_map(|value| match value {
            hirsel_tir::Rvalue::ReadConstant { definition, arguments } if !arguments.is_empty() => Some(definition.clone()),
            _ => None,
        }).unwrap();
        let signature = seed.item_signature(&declaration).unwrap().value.fingerprint;
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None;
        fresh.adapters.lowering = None;
        let actual = fresh.lower_to_tir(&path).unwrap();
        assert_eq!(actual.value.path, expected.value.path);
        assert_eq!(fresh.item_signature(&declaration).unwrap().value.fingerprint, signature);
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
        assert!(remote.lower_to_tir(&path).is_err(), "declaration bytes cannot transfer V completion");
        let mut edited = signature_session(store.clone(), &source.replace("field:u32", "field:u64"), true);
        edited.adapters.modes = None;
        edited.adapters.lowering = None;
        assert!(edited.lower_to_tir(&path).is_err(), "private V universe changes invalidate completion");
        let mut oracle = signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
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
            let hidden = lower.value.bodies[lower.value.entry.0 as usize].rvalues.iter().find_map(|r| match r {
                hirsel_tir::Rvalue::ReadConstant { definition, arguments } if !arguments.is_empty() => Some(definition.clone()), _ => None,
            }).unwrap();
            let expected = seed.materialize_values(roots.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(expected.iter().all(Result::is_ok), "jobs={jobs}: {expected:?}");
            let mut fresh = signature_session(store.clone(), source, false);
            fresh.adapters.modes = None;
            fresh.adapters.lowering = None;
            let actual = fresh.materialize_values(roots.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(actual.iter().all(Result::is_ok), "jobs={jobs}: {actual:?}");
            assert_eq!(actual.iter().map(|v| &v.as_ref().unwrap().root.kind).collect::<Vec<_>>(), expected.iter().map(|v| &v.as_ref().unwrap().root.kind).collect::<Vec<_>>());
            assert_eq!(fresh.constant_instance(&hidden, &[2]).unwrap().root.kind, hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)));
            assert!(fresh.constant_instance(&hidden, &[3]).is_err(), "only selected instances replay");
            for root in &roots { assert_eq!(fresh.recompute_count(root, QueryKind::Modes), 0); }
            let copied = Arc::new(store::MemStore::default());
            for (key, bytes) in store.take() {
                store::ResultStore::put(&*copied, &key, &bytes);
                store::ResultStore::put(&*store, &key, &bytes);
            }
            let mut remote = signature_session(copied, source, false);
            remote.adapters.modes = None; remote.adapters.lowering = None;
            assert!(remote.materialize_values(roots.clone(), hirsel_intern::jobs::Jobs::new(jobs)).iter().all(Result::is_err), "producer transactions cannot transfer private completion");
            let mut oracle = signature_session(store.clone(), source, true).with_verify_store(store::VerifyStore::Recompute);
            oracle.register_staged_definition(&definition("scaled")).unwrap();
            assert_eq!(oracle.materialize_constant(&definition("B")).unwrap(), hirsel_tir::Constant::Integer(5), "worker/scalar transaction encoding jobs={jobs}");
            let mut changed = signature_session(store, &source.replace("n*n", "n+n"), true);
            changed.adapters.modes = None; changed.adapters.lowering = None;
            assert!(changed.materialize_constant(&definition("B")).is_err(), "stage owner body changes invalidate enclosing values");
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
        let hidden = generic.value.bodies[generic.value.entry.0 as usize].rvalues.iter().find_map(|r| match r {
            hirsel_tir::Rvalue::ReadConstant { definition, arguments } if !arguments.is_empty() => Some(definition.clone()), _ => None,
        }).unwrap();
        let expected = seed.lower_to_tir(&root).unwrap();
        let mut fresh = signature_session(store.clone(), source, false);
        fresh.adapters.modes = None; fresh.adapters.lowering = None;
        let actual = fresh.lower_to_tir(&root).unwrap();
        assert_eq!(actual.value.path, expected.value.path);
        assert_eq!(fresh.lower_adapter_calls(&root), 0);
        assert_eq!(fresh.recompute_count(&root, QueryKind::Modes), 0);
        assert_eq!(fresh.constant_instance(&hidden, &[2]).unwrap().root.kind, hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)));
        assert!(fresh.constant_instance(&hidden, &[3]).is_err(), "unselected size stays absent");
        fresh.lower_to_tir(&root).unwrap();
        assert_eq!(fresh.lower_adapter_calls(&root), 0, "resident Lower replays the same ledger");
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, false);
        remote.adapters.modes = None; remote.adapters.lowering = None;
        assert!(remote.lower_to_tir(&root).is_err(), "producer ledger cannot transfer V completion");
        let mut oracle = signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
        oracle.register_staged_definition(&definition("scaled")).unwrap();
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
        fresh.adapters.modes = None; fresh.adapters.lowering = None;
        fresh.check_definition(&path).unwrap();
        assert_eq!(fresh.generated_source(&path).unwrap().text, expected);
        assert_eq!(fresh.lower_adapter_calls(&definition("text")), 0, "generator helper was not executed");
        assert_eq!(fresh.recompute_count(&path, QueryKind::Modes), 0);
        let copied = Arc::new(store::MemStore::default());
        for (key, bytes) in store.take() {
            store::ResultStore::put(&*copied, &key, &bytes);
            store::ResultStore::put(&*store, &key, &bytes);
        }
        let mut remote = signature_session(copied, source, true);
        remote.adapters.modes = None; remote.adapters.lowering = None;
        assert!(remote.check_definition(&path).is_err(), "producer text cannot transfer private generator completion");
        let changed = source.replace("7u32", "9u32");
        let mut invalid = signature_session(store.clone(), &changed, true);
        invalid.adapters.modes = None; invalid.adapters.lowering = None;
        assert!(invalid.check_definition(&path).is_err(), "executed helper changes invalidate text");
        let mut private = signature_session(store.clone(), &source.replace("field:u32", "field:u64"), true);
        private.adapters.modes = None; private.adapters.lowering = None;
        assert!(private.check_definition(&path).is_err(), "private V universe changes invalidate text completion");
        let off_store = Arc::new(store::MemStore::default());
        let mut off = signature_session(off_store.clone(), source, true);
        off.verify = Verify::Off;
        off.check_definition(&path).unwrap();
        let mut on = signature_session(off_store, source, true);
        on.adapters.modes = None; on.adapters.lowering = None;
        assert!(on.check_definition(&path).is_err(), "VerifyOff generator text cannot mint completion");
        let mut oracle = signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
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
            let outcomes = seed.check_lower_definition_prefix(vec![definition("FIRST"), definition("BAD"), definition("later")], hirsel_intern::jobs::Jobs::new(jobs));
            assert_eq!(outcomes.len(), 2, "jobs={jobs}");
            assert!(outcomes[0].result.is_ok(), "jobs={jobs}: {:?}", outcomes[0].result);
            assert!(outcomes[1].result.is_err());
            assert!(seed.generated_source(&definition("later")).is_none(), "speculative suffix remains unpublished");
            let expected = seed.generated_source(&definition("value")).unwrap().text.clone();
            let mut fresh = signature_session(store.clone(), source, true);
            fresh.adapters.modes = None; fresh.adapters.lowering = None;
            fresh.check_definition(&definition("value")).unwrap();
            assert_eq!(fresh.generated_source(&definition("value")).unwrap().text, expected);
            assert!(fresh.check_definition(&definition("later")).is_err(), "suffix text not persisted at jobs={jobs}");
            let mut oracle = signature_session(store, source, true).with_verify_store(store::VerifyStore::Recompute);
            oracle.check_definition(&definition("value")).unwrap();
            assert_eq!(oracle.generated_source(&definition("value")).unwrap().text, expected);
        }
    });
}
