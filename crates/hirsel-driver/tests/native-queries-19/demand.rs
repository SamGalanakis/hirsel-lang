use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::queries::{declarations::NativeDeclarations, demand::DemandInstances};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[test]
fn actual_demand_instance_graph_hits_before_recipe_or_value_construction() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let source = "resource type R[T:copyable] {n:T} drop(x) {let _=x.n}\nfn outer[T](take x:T)->T {let f:once fn()->T=fn()->T{take x};f()}\nfn unrelated()->u64{13u64}\npub fn main()->u64 {let r=R(n:1u64);outer(take 7u64)}";
        let session = |source: &str| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("demand.hir", source.into()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            engine
        };
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"actual demand graph control");
        let run = |engine: &mut QueryEngine, disable_lowering: bool| {
            if disable_lowering {
                engine.adapters.lowering = None;
            }
            let mut source = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let mut declarations = NativeDeclarations::collect(engine, &mut source, &[]).unwrap();
            if disable_lowering {
                assert_eq!(source.counts.body_demands, 0);
            }
            assert!(declarations.program.definitions.is_empty());
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let mut graph = DemandInstances::new(engine, store.clone(), target.clone(), policy);
            let mut pending = graph.roots(engine, &declarations).unwrap();
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default()
            );
            let mut seen = BTreeSet::new();
            let mut results = BTreeMap::new();
            while let Some(key) = pending.pop() {
                if !seen.insert(key.instance.encoding().to_vec()) {
                    continue;
                }
                let output = graph
                    .instance(engine, &mut declarations, &key, &options)
                    .unwrap();
                assert_eq!(
                    output.product.product.address,
                    key.address(native::queries::INSTANCE_KIND)
                );
                pending.extend(output.dependencies.iter().cloned());
                results.insert(
                    key.instance.encoding().to_vec(),
                    output.product.product.record,
                );
            }
            (
                results,
                graph.imports,
                graph.recipe_decodes,
                trace.snapshot(),
                trace.totals(),
            )
        };
        let mut first = session(source);
        let (expected, imports, decodes, _, _) = run(&mut first, false);
        assert!(
            imports >= 5,
            "source, generic, closure and finalizer misses ran"
        );
        assert_eq!(imports, decodes);
        drop(first);
        let mut fresh = session(source);
        let (hits, imports, decodes, _, counts) = run(&mut fresh, true);
        assert_eq!(hits, expected);
        assert_eq!((imports, decodes), (0, 0));
        assert_eq!(counts, hirsel_lir::construction::ValueCounts::default());
        drop(fresh);
        let changed = source.replace("13u64", "14u64");
        let mut edit = session(&changed);
        let (edited, imports, decodes, owners, _) = run(&mut edit, false);
        assert_eq!((imports, decodes), (1, 1));
        let mut changed_outputs = 0;
        for (key, old) in expected {
            if edited[&key] != old {
                changed_outputs += 1;
            }
        }
        assert_eq!(changed_outputs, 1);
        assert!(owners.keys().all(|(path, _)| path.0.name == "unrelated"));
    });
}

#[test]
fn actual_library_graph_uses_admitted_environment_products_on_fresh_hits() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        const SOURCE: &str = "fn step(mut n:u64)->Option[u64] {if n==2u64 {None} else {let x=n;n+=1u64;Some(x)}}\npub fn main()->u64 {let state=0u64;let mut it=iter.from(take state,step);let _=iter.next(mut it);0u64}";
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"library demand graph control");
        let mut expected = BTreeMap::new();
        for fresh in [false, true] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("library-demand.hir", SOURCE.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            if fresh {
                engine.adapters.lowering = None;
            }
            let mut source = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let mut declarations =
                NativeDeclarations::collect(&mut engine, &mut source, &[]).unwrap();
            if fresh {
                assert_eq!(source.counts.body_demands, 0);
            }
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let mut graph = DemandInstances::new(&engine, store.clone(), target.clone(), policy);
            let mut pending = graph.roots(&engine, &declarations).unwrap();
            let mut seen = BTreeSet::new();
            let mut actual = BTreeMap::new();
            while let Some(key) = pending.pop() {
                if !seen.insert(key.instance.encoding().to_vec()) {
                    continue;
                }
                let output = graph
                    .instance(&mut engine, &mut declarations, &key, &options)
                    .unwrap();
                pending.extend(output.dependencies.iter().cloned());
                actual.insert(
                    key.instance.encoding().to_vec(),
                    output.product.product.record,
                );
            }
            if fresh {
                assert_eq!(actual, expected);
                assert_eq!((graph.imports, graph.recipe_decodes), (0, 0));
                assert_eq!(
                    trace.totals(),
                    hirsel_lir::construction::ValueCounts::default()
                );
            } else {
                assert!(
                    trace
                        .snapshot()
                        .keys()
                        .any(|(path, _)| path.0.name == "iter_from_step")
                );
                assert!(graph.imports > 2);
                expected = actual;
            }
        }
    });
}

#[test]
fn ct5_selection_reads_recheck_before_native_hit_without_decoding_unchanged_values() {
    with_compiler_stack(|| {
        const SOURCE: &str = "type P {n:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String] {Ok(codegen.body(\"fn(v:standalone@0/P)->u64 uses {} {v.n}\"))}\nfn chosen[T](v:T)->u64 uses {} = comptime gen(shape(T))\nfn alternate(v:P)->u64 uses {} {v.n+1u64}\npub fn main()->u64 uses {} {chosen(P(n:3u64))}";
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let session = || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine
                .sources
                .add("selection-demand.hir", SOURCE.into())
                .unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            engine
        };
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let options = native::Options::new(target.clone());
        let policy = ContentHash::of_bytes(b"CT5 current selected native body");
        let main = entry_path(&default_context());
        let mut alternate = main.clone();
        alternate.0.name = "alternate".into();
        let mut chosen = main.clone();
        chosen.0.name = "chosen".into();
        let mut first = session();
        let mut discovery = NativeDiscoveryCache::new(first.types().clone(), store.clone());
        let mut declarations =
            NativeDeclarations::collect(&mut first, &mut discovery, &[]).unwrap();
        let mut graph = DemandInstances::new(&first, store.clone(), target.clone(), policy);
        let root = graph
            .roots(&first, &declarations)
            .unwrap()
            .into_iter()
            .find(|r| r.instance.definition() == &main)
            .unwrap();
        let original = graph
            .instance(&mut first, &mut declarations, &root, &options)
            .unwrap();
        assert!(
            original
                .selection_reads
                .iter()
                .any(|read| read.declaration == chosen
                    && read.generated
                    && read.selected.is_some())
        );
        let original_product = original.product.product;
        drop(graph);
        drop(declarations);
        drop(first);

        let mut fresh = session();
        let mut discovery = NativeDiscoveryCache::new(fresh.types().clone(), store.clone());
        let mut declarations =
            NativeDeclarations::collect(&mut fresh, &mut discovery, &[]).unwrap();
        assert_eq!(discovery.counts.body_demands, 0);
        // Source CT-5 generation performs its actual source-query work first.
        // Native admission below has no lowering adapter to reconstruct a body.
        fresh.adapters.lowering = None;
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let mut graph = DemandInstances::new(&fresh, store.clone(), target.clone(), policy);
        graph.roots(&fresh, &declarations).unwrap();
        let hit = graph
            .instance(&mut fresh, &mut declarations, &root, &options)
            .unwrap();
        assert_eq!(hit.product.product, original_product);
        assert_eq!((graph.imports, graph.recipe_decodes), (0, 0));
        assert_eq!(
            trace.totals(),
            hirsel_lir::construction::ValueCounts::default()
        );
        drop(graph);
        let declaration = hirsel_syntax::DefId::intern(&chosen);
        for row in declarations
            .program
            .instance_bodies
            .instances
            .get_mut(&declaration)
            .unwrap()
            .values_mut()
        {
            row.definition = hirsel_syntax::DefId::intern(&alternate);
        }
        // The replacement is independently well typed at the original generic
        // declaration. Only native selection changed, not source V or headers.
        assert!(
            hirsel_validate::validate_instance_catalog(
                &declarations.program.types,
                &declarations.program.instance_bodies,
                &declarations.program.signatures,
                &declarations.context.sources.keys().cloned().collect()
            )
            .is_empty()
        );
        let mut graph = DemandInstances::new(&fresh, store.clone(), target.clone(), policy);
        graph.roots(&fresh, &declarations).unwrap();
        let changed = graph
            .instance(&mut fresh, &mut declarations, &root, &options)
            .unwrap();
        assert_eq!((graph.imports, graph.recipe_decodes), (1, 1));
        assert_ne!(changed.product.product.action, original_product.action);
        assert_ne!(changed.product.product.record, original_product.record);
        assert!(
            changed.selection_reads.iter().any(
                |read| read.declaration == chosen && read.selected.as_ref() == Some(&alternate)
            )
        );
        assert!(
            changed
                .dependencies
                .iter()
                .any(|dependency| dependency.instance.definition() == &alternate)
        );
        assert!(trace.snapshot().keys().all(|(path, _)| path == &main));
        drop(graph);
        declarations
            .program
            .instance_bodies
            .instances
            .remove(&declaration);
        let mut graph = DemandInstances::new(&fresh, store, target, policy);
        graph.roots(&fresh, &declarations).unwrap();
        let error = graph
            .instance(&mut fresh, &mut declarations, &root, &options)
            .err()
            .expect("a missing current selection cannot reuse old native C");
        assert!(error.contains("Unselected"), "{error}");
        assert_eq!(graph.imports, 0);
    });
}

#[test]
fn generated_lower_cutoff_refuses_missing_generation_and_changed_text_with_equal_headers() {
    use hirsel_engine::store::ResultStore;
    with_compiler_stack(|| {
        let source = |expression: &str| {
            format!(
                "type P {{n:u64}}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {{}}],String] {{Ok(codegen.body(\"fn(v:standalone@0/P)->u64 uses {{}} {{{expression}}}\"))}}\nfn chosen[T](v:T)->u64 uses {{}} = comptime gen(shape(T))\npub fn main()->u64 uses {{}} {{chosen(P(n:3u64))}}"
            )
        };
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let session = |source: String| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("generated-lower.hir", source).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            engine
        };
        let mut first = session(source("v.n"));
        let mut discovery = NativeDiscoveryCache::new(first.types().clone(), store.clone());
        let declarations = NativeDeclarations::collect(&mut first, &mut discovery, &[]).unwrap();
        let selected = declarations
            .sources
            .keys()
            .find(|path| path.0.name.starts_with("chosen$instance$"))
            .unwrap()
            .clone();
        let original = &declarations.sources[&selected];
        let header = original.signature.fingerprint;
        let generated = original.source.generated.as_ref().unwrap().text.clone();
        let output = original
            .lower
            .clone()
            .expect("actual independently verified generated Lower");
        assert!(first.materialize_verified_lower_output(&output).is_some());
        drop(declarations);
        drop(first);

        let mut missing = session(source("v.n"));
        assert!(missing.generated_source(&selected).is_none());
        assert!(missing.verified_lower_output(&selected).unwrap().is_none());
        assert!(missing.materialize_verified_lower_output(&output).is_none());
        assert_eq!(missing.recompute_count(&selected, QueryKind::Lower), 0);
        drop(missing);

        let mut same = session(source("v.n"));
        same.native_source_roots(&[]).unwrap();
        let current = same.native_definition_product(&selected).unwrap();
        assert_eq!(current.signature.fingerprint, header);
        assert_eq!(current.source.generated.as_ref().unwrap().text, generated);
        let reused = current
            .lower
            .expect("current generation admits the existing V-owned envelope");
        assert_eq!(reused.reference(), output.reference());
        assert_eq!(reused.observation_identity(), output.observation_identity());
        assert_eq!(same.recompute_count(&selected, QueryKind::Lower), 0);
        drop(same);

        let mut changed = session(source("v.n+1u64"));
        changed.native_source_roots(&[]).unwrap();
        let current = changed.native_definition_product(&selected).unwrap();
        assert_eq!(current.signature.fingerprint, header);
        assert_ne!(current.source.generated.as_ref().unwrap().text, generated);
        // Source CT-5 generation has completed its first Lower/V for the new
        // literal. It may publish that fresh envelope; the previous envelope
        // must never become the current source through its unchanged header.
        let replacement = current
            .lower
            .expect("changed generation completed actual source Lower/V");
        assert_ne!(
            replacement.reference(),
            output.reference(),
            "old V bytes do not identify current generated source"
        );
        assert_ne!(
            replacement.observation_identity(),
            output.observation_identity()
        );
        assert_eq!(
            changed
                .verified_lower_output(&selected)
                .unwrap()
                .unwrap()
                .reference(),
            replacement.reference()
        );
        assert!(
            changed
                .materialize_verified_lower_output(&replacement)
                .is_some()
        );
        assert!(
            store.get_content(output.reference()).is_some(),
            "the old envelope remains intact; refusal is not eviction"
        );
        assert!(
            changed.materialize_verified_lower_output(&output).is_none(),
            "generated anchor identity must match current text"
        );
        assert_eq!(changed.recompute_count(&selected, QueryKind::Lower), 1);
    });
}
