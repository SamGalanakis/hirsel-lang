//! Durable completion consumes compact facts without loading the Modes body.
use hirsel_driver::*;
use hirsel_engine::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Observed {
    inner: store::MemStore,
    reads: AtomicUsize,
    writes: AtomicUsize,
}
impl store::ResultStore for Observed {
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        store::ResultStore::get(&self.inner, key)
    }
    fn put(&self, key: &store::EntryKey, bytes: &[u8]) {
        self.writes.fetch_add(1, Ordering::Relaxed);
        store::ResultStore::put(&self.inner, key, bytes);
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
            store.writes.store(0, Ordering::Relaxed);
            let (mut fresh, actual) = publish(store.clone(), source, jobs);
            assert!(actual.iter().all(|o| o.result.is_ok()));
            assert_eq!(
                store.reads.load(Ordering::Relaxed),
                2,
                "one compact record per definition; no Modes lookup"
            );
            assert_eq!(store.writes.load(Ordering::Relaxed), 0);
            assert_eq!(fresh.store_adoptions(), 2);
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
            let (invalidated_engine, invalidated) = publish(store.clone(), &changed, jobs);
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
