use hirsel_driver::*;
use hirsel_engine::*;
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
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[test]
fn source_root_descriptors_link_both_backends_and_restore_without_ready_body_reads() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(store::DirStore::new(directory.path().join("products")));
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let tools =
            native::Toolchain::in_tree_diagnostic(&directory.path().join("runtime"), &[&target])
                .unwrap();
        let runtime = native::object_queries::store_link_input(
            store.as_ref(),
            &std::fs::read(tools.archive(&target.triple).unwrap()).unwrap(),
        )
        .unwrap();
        let policy = ContentHash::of_bytes(b"source root native integration");
        let options = native::Options::new(target.clone());
        let mut previous: Option<(store::Reference, store::Reference)> = None;
        let mut reference = None;
        let mut previous_ready: Option<
            BTreeMap<(hirsel_syntax::DefPath, ContentHash), store::Reference>,
        > = None;
        for (literal, fresh) in [(7u64, false), (7, true), (8, false), (8, true)] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("root-demand.hir", format!("const SECOND:u64=comptime {literal}u64\npub fn main()->Result[(),u64] {{ Error(SECOND) }}\n")).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let main = entry_path(&default_context());
            if !fresh {
                let execution = run_observed_snapshot(
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
                    execution.observation.as_ref().unwrap(),
                ));
            } else {
                engine.adapters.lowering = None;
            }
            let mut discoveries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
            let mut declarations =
                NativeDeclarations::collect(&mut engine, &mut discoveries, &[]).unwrap();
            let source = Arc::new(NativeLinkSource::capture(&mut engine, &declarations).unwrap());
            let trace = hirsel_lir::construction::ValueTrace::default();
            let _observe = trace.enter();
            let mut instances =
                DemandInstances::new(&engine, store.clone(), target.clone(), policy);
            let mut pending = instances.roots(&engine, &declarations).unwrap();
            let mut seen = BTreeSet::new();
            let mut nodes = Vec::new();
            while let Some(key) = pending.pop() {
                if !seen.insert(key.instance.encoding().to_vec()) {
                    continue;
                }
                let output = instances
                    .instance(&mut engine, &mut declarations, &key, &options)
                    .unwrap();
                pending.extend(output.dependencies.iter().cloned());
                nodes.push(ArenaNode {
                    equations: EquationProduct {
                        owner: key,
                        reference: output.product.local_equations,
                    },
                    callees: output.callees.clone(),
                });
            }
            nodes.sort_by(|a, b| a.equations.owner.canonical_cmp(&b.equations.owner));
            let arena = settle_arena_queries(
                store.as_ref(),
                &nodes,
                &[],
                0,
                &mut ArenaQueryCounts::default(),
            )
            .unwrap();
            let operations = declarations.program.operation_registry();
            let manifest = hirsel_core_contract::Manifest::skeleton(
                declarations.program.operations.values().cloned(),
            );
            let emission = EmissionOptions {
                input: declarations.program.input.clone(),
                opt: hirsel_lir::emission::OptLevel::None,
                debug: true,
                callbacks: false,
                tracking: false,
                foreign: Default::default(),
                inline_instances: Default::default(),
                verify_backend: true,
                measurement: false,
                fuel: false,
            };
            if fresh {
                assert_eq!((instances.imports, instances.recipe_decodes), (0, 0));
            }
            let snapshot = NativeSnapshot::new(
                store.clone(),
                instances,
                arena,
                options.clone(),
                emission,
                manifest,
                Arc::new(operations),
            )
            .unwrap();
            drop(declarations);
            drop(engine);
            let key = NativeRootKey {
                roots: vec![main.clone()],
                target_abi: target.digest(),
                policy,
            };
            let before = trace.totals();
            let mut roots =
                RootProducts::new(snapshot.clone(), source.clone(), key.clone()).unwrap();
            let root = roots.root(&key).unwrap();
            assert_eq!(trace.totals(), before);
            assert_eq!(
                snapshot.ready_materializations(),
                0,
                "root publication does not read Ready bodies"
            );
            if let Some((statics, launch)) = previous {
                if fresh {
                    assert_eq!(
                        (root.static_manifest, root.launch_manifest),
                        (statics, launch)
                    );
                } else {
                    assert_ne!(root.static_manifest, statics);
                    assert_ne!(root.launch_manifest, launch);
                }
            }
            previous = Some((root.static_manifest, root.launch_manifest));
            let mut objects = ObjectQueries::new(
                store.clone(),
                roots,
                |_: &LinkKey,
                 _: &LinkInputs,
                 _: &[hirsel_lir::stage::ObjectArtifact]|
                 -> Result<Vec<u8>, String> {
                    Err("link view is selected after codegen".into())
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
                        profile_options: ContentHash::of_bytes(b"debug opt none"),
                        runtime_abi: ContentHash::of_bytes(
                            &hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes(),
                        ),
                    };
                    fragments[index].push((*objects.codegen(&key).unwrap()).clone());
                }
            }
            let mut roots = objects.native;
            let ready = root
                .instances
                .iter()
                .map(|owner| roots.ready(owner).unwrap())
                .collect::<Vec<_>>();
            assert!(roots.with_link_context(&root, &[], |_| Ok(())).is_err());
            let mut wrong = (*root).clone();
            wrong.launch_manifest = root.static_manifest;
            assert!(
                roots
                    .with_link_context(&Arc::new(wrong), &ready, |_| Ok(()))
                    .is_err()
            );
            let cold = previous_ready.is_none();
            let ready_records = root
                .instances
                .iter()
                .zip(&ready)
                .map(|(owner, product)| {
                    (
                        (owner.instance.definition().clone(), owner.instance.digest()),
                        product.product.record,
                    )
                })
                .collect::<BTreeMap<_, _>>();
            if let Some(previous) = &previous_ready {
                let changed = ready_records
                    .iter()
                    .filter(|(owner, record)| previous.get(owner) != Some(record))
                    .map(|(owner, _)| owner.0.clone())
                    .collect::<Vec<_>>();
                assert_eq!(
                    changed,
                    if fresh { vec![] } else { vec![main.clone()] },
                    "the static edit changes only the owning Ready descriptor"
                );
            }
            previous_ready = Some(ready_records);
            let before_link = trace.totals();
            let body_reads = snapshot.ready_materializations();

            assert_eq!(
                body_reads,
                if fresh {
                    0
                } else if cold {
                    2 * root.instances.len() as u64
                } else {
                    2
                }
            );
            for (index, backend) in ["llvm", "cranelift"].into_iter().enumerate() {
                let dir = directory
                    .path()
                    .join(format!("{literal}-{fresh}-{backend}"));
                std::fs::create_dir_all(&dir).unwrap();
                let mut link_view = roots.link_view().unwrap();
                let calls = std::cell::Cell::new(0u32);
                let mut linker = ObjectQueries::new(
                    store.clone(),
                    roots,
                    |_: &LinkKey, _: &LinkInputs, objects: &[hirsel_lir::stage::ObjectArtifact]| {
                        calls.set(calls.get() + 1);
                        link_view.link(
                            &root,
                            &ready,
                            RootLinkInputs {
                                entry: &main,
                                backend: if backend == "llvm" {
                                    native::Backend::Llvm
                                } else {
                                    native::Backend::Cranelift
                                },
                                objects,
                                toolchain: &tools,
                                directory: &dir,
                            },
                        )
                    },
                );
                let inputs = LinkInputs {
                    fragments: fragments[index].clone(),
                    static_manifest: root.static_manifest,
                    runtime_archives: vec![runtime],
                    foreign_inputs: vec![],
                    launch_manifest: root.launch_manifest,
                };
                let key = LinkKey {
                    roots: root.instances.clone(),
                    target: target.digest(),
                    linker_options: ContentHash::of_parts(
                        "selected root integration linker",
                        [
                            backend.as_bytes(),
                            tools.lld.as_os_str().as_encoded_bytes(),
                            tools.kit.as_os_str().as_encoded_bytes(),
                        ],
                    ),
                };
                let linked = linker.link(&key, &inputs).unwrap();
                assert_eq!(linker.link(&key, &inputs).unwrap().product, linked.product);
                assert_eq!(
                    calls.get(),
                    if fresh { 0 } else { 1 },
                    "Link hits perform no linker-context assembly"
                );
                let executable = dir.join("restored");
                native::object_queries::materialize_executable(
                    store.as_ref(),
                    &linked,
                    &executable,
                )
                .unwrap();
                let run = native::launch(&executable, &target, &tools, &dir, true).unwrap();
                roots = linker.native;
                // Observation is an explicit metadata-only diagnostic consumer,
                // separate from the Link hit above; it opens no Ready body.
                roots
                    .with_link_context(&root, &ready, |program| {
                        assert!(program.units.is_empty());
                        let actual = native::observe(&run, program, &source.sites())
                            .map_err(|e| e.to_string())?;
                        assert_eq!(
                            Some(&actual),
                            reference.as_ref(),
                            "{literal} {fresh} {backend}"
                        );

                        Ok(())
                    })
                    .unwrap();
                assert!(
                    String::from_utf8_lossy(&run.stderr).contains(&literal.to_string()),
                    "actual source error report: {run:?}"
                );
            }
            assert_eq!(
                snapshot.ready_materializations(),
                body_reads,
                "link and observations decode no Ready bodies"
            );
            assert_eq!(trace.totals(), before_link);
            if fresh {
                assert_eq!(trace.totals(), Default::default());
            }
        }
    });
}
