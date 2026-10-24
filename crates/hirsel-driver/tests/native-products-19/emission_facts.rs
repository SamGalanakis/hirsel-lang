use hirsel_driver::*;
use hirsel_engine::{store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::object_queries::{CodegenKey, LinkInputs, LinkKey, ObjectProducts, ObjectQueries};
use native::queries::{
    NativeProducts, NativeRootKey, NativeRoots,
    arena::{ArenaQueryCounts, EquationProduct},
    declarations::NativeDeclarations,
    demand::DemandInstances,
    graph::{ArenaNode, settle_arena_queries},
    products::{EmissionOptions, NativeSnapshot},
    root_source::NativeLinkSource,
    roots::{RootLinkInputs, RootProducts},
    source_inputs::NativeInstanceInputs,
};
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn admitted_graph_derives_callback_recovery_and_inline_emission_facts_without_values_on_hits() {
    with_compiler_stack(|| {
        for (callback, tracking) in [(false, false), (true, false), (false, true), (true, true)] {
            run(callback, tracking);
        }
    });
}
fn run(callback: bool, tracking: bool) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store::DirStore::new(directory.path().join("products")));
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let tools = native::Toolchain::in_tree_with_products(
        &directory.path().join("runtime"),
        &[&target],
        &[native::RuntimeBuild {
            diagnostic: true,
            ..Default::default()
        }],
        None,
        store.clone(),
    )
    .unwrap();
    let archive = native::object_queries::store_link_input(
        store.as_ref(),
        &std::fs::read(tools.archive(&target.triple).unwrap()).unwrap(),
    )
    .unwrap();
    let policy = ContentHash::of_bytes(b"actual emission graph flags");
    let mut previous_fragments = None;
    let mut reference = None;
    for fresh in [false, true] {
        let callback_source = if callback {
            "type Tally {n:i32}\nfn step(mut state:Tally,x:i32)->i32 {state.n+=1;x}\nfn form()->FnPtr[fn(mut state:unique Tally,x:i32)->i32] {step}\n"
        } else {
            ""
        };
        let recover_source = if tracking {
            "fn bounded(cap:u64)->Result[u64,OutOfSpace] {arena.bounded(cap,fn(a:Arena)->u64 {let xs:List[u8] in a=[7u8];list.len(xs)})}\n"
        } else {
            ""
        };
        let prefix = if callback { "let _=form();" } else { "" };
        let value = if tracking {
            "match bounded(64u64) {Ok(n)=>n,Error(_)=>0u64}"
        } else {
            "7u64"
        };
        let text = format!(
            "{callback_source}{recover_source}pub fn main()->Result[(),u64] {{{prefix}Error({value})}}"
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine.sources.add("emission-facts.hir", text).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let main = entry_path(&default_context());
        if !fresh {
            let run = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: main.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            reference = Some(native::Observation::of_reference(
                run.observation.as_ref().unwrap(),
            ));
        } else {
            engine.adapters.lowering = None;
        }
        let mut discovery = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let declarations = NativeDeclarations::collect(&mut engine, &mut discovery, &[]).unwrap();
        let source = Arc::new(
            NativeLinkSource::capture_for_entry(&mut engine, &declarations, &main).unwrap(),
        );
        let inputs =
            NativeInstanceInputs::complete(&mut engine, declarations, store.clone()).unwrap();
        drop(engine);
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _guard = trace.enter();
        let options = native::Options::new(target.clone());
        let mut provider = inputs.provider();
        let mut instances =
            DemandInstances::new_from_source(&provider, store.clone(), target.clone(), policy);
        let mut pending = instances.roots_from(&provider).unwrap();
        let mut seen = BTreeSet::new();
        let mut completed = Vec::new();
        while let Some(owner) = pending.pop() {
            if !seen.insert(owner.instance.encoding().to_vec()) {
                continue;
            }
            let output = instances
                .instance_from(&mut provider, &owner, &options)
                .unwrap();
            pending.extend(output.dependencies.iter().cloned());
            completed.push((owner, output));
        }
        completed.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        assert_eq!(
            instances.imports,
            if fresh { 0 } else { completed.len() as u64 }
        );
        let callbacks = completed
            .iter()
            .flat_map(|(_, output)| output.emission.callbacks.iter().cloned())
            .collect::<BTreeSet<_>>();
        assert_eq!(!callbacks.is_empty(), callback);
        if callback {
            assert!(callbacks.iter().any(|path| path.0.name == "step"));
        }
        assert_eq!(
            completed.iter().any(|(_, output)| output.emission.tracking),
            tracking
        );
        let nodes = completed
            .iter()
            .map(|(owner, output)| ArenaNode {
                equations: EquationProduct {
                    owner: owner.clone(),
                    reference: output.product.local_equations,
                },
                callees: output.callees.clone(),
            })
            .collect::<Vec<_>>();
        let arena = settle_arena_queries(
            store.as_ref(),
            &nodes,
            &[],
            0,
            &mut ArenaQueryCounts::default(),
        )
        .unwrap();
        let mut request = EmissionOptions::request(
            inputs.declarations.program.input.clone(),
            native::OptLevel::Default,
            false,
            Default::default(),
            true,
            false,
            false,
        );
        // These are derived outputs rather than host-controlled switches. A
        // legacy caller's guesses are replaced before any context publication.
        request.callbacks = !callback;
        request.tracking = !tracking;
        request
            .inline_instances
            .insert(ContentHash::of_bytes(b"not an admitted template"));
        let operations = inputs.declarations.program.operation_registry();
        let manifest = inputs.declarations.manifest().unwrap();
        let before = trace.totals();
        let snapshot = NativeSnapshot::new(
            store.clone(),
            instances,
            arena,
            options,
            request,
            manifest,
            Arc::new(operations),
        )
        .unwrap();
        assert_eq!(trace.totals(), before);
        assert_eq!(snapshot.emission().callbacks, callback);
        assert_eq!(snapshot.emission().tracking, tracking);
        assert!(snapshot.emission().inline_instances.is_empty());
        let root_key = NativeRootKey {
            roots: vec![main.clone()],
            target_abi: target.digest(),
            policy,
        };
        let mut roots =
            RootProducts::new(snapshot.clone(), source.clone(), root_key.clone()).unwrap();
        let root = roots.root(&root_key).unwrap();
        let mut objects = ObjectQueries::new(
            store.clone(),
            roots,
            |_: &LinkKey, _: &LinkInputs, _: &[hirsel_lir::stage::ObjectArtifact]| {
                Err("Codegen scope".to_string())
            },
        );
        let mut fragments = [Vec::new(), Vec::new()];
        for (index, backend) in ["llvm", "cranelift"].into_iter().enumerate() {
            for owner in &root.instances {
                let key = CodegenKey {
                    owner: owner.clone(),
                    backend: backend.into(),
                    backend_version: ContentHash::of_parts(
                        "backend",
                        [backend.as_bytes(), COMPILER_IDENTITY.as_slice()],
                    ),
                    target_features: target.digest(),
                    profile_options: ContentHash::of_bytes(b"default opt derived flags"),
                    runtime_abi: ContentHash::of_bytes(
                        &hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes(),
                    ),
                };
                fragments[index].push((*objects.codegen(&key).unwrap()).clone());
            }
        }
        let records = fragments
            .iter()
            .map(|outputs| {
                outputs
                    .iter()
                    .map(|output| output.product.record)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if let Some(old) = previous_fragments {
            assert_eq!(records, old);
        }
        previous_fragments = Some(records);
        let mut roots = objects.native;
        let ready = root
            .instances
            .iter()
            .map(|owner| roots.ready(owner).unwrap())
            .collect::<Vec<_>>();
        if fresh {
            assert_eq!(trace.totals(), Default::default());
            assert_eq!(snapshot.ready_materializations(), 0);
        }
        let before = trace.totals();
        let body_reads = snapshot.ready_materializations();
        for (index, backend) in [native::Backend::Llvm, native::Backend::Cranelift]
            .into_iter()
            .enumerate()
        {
            let output = directory.path().join(format!("{}-{fresh}", backend.name()));
            std::fs::create_dir_all(&output).unwrap();
            let objects = fragments[index]
                .iter()
                .map(|fragment| {
                    native::object_queries::load_object(store.as_ref(), fragment.object).unwrap()
                })
                .collect::<Vec<_>>();
            let mut view = roots.link_view().unwrap();
            let executable = view
                .link(
                    &root,
                    &ready,
                    RootLinkInputs {
                        entry: &main,
                        backend,
                        objects: &objects,
                        toolchain: &tools,
                        directory: &output,
                    },
                )
                .unwrap();
            let path = output.join("program");
            std::fs::write(&path, executable).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            let run = native::launch(&path, &target, &tools, &output, true).unwrap();
            roots
                .with_link_context(&root, &ready, |program| {
                    assert!(program.units.is_empty());
                    assert_eq!(!program.callbacks.is_empty(), callback);
                    assert_eq!(program.tracking(), tracking);
                    let observed = native::observe(&run, program, &source.sites())
                        .map_err(|error| error.to_string())?;
                    assert_eq!(Some(&observed), reference.as_ref());
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(trace.totals(), before);
        assert_eq!(snapshot.ready_materializations(), body_reads);
    }
}
