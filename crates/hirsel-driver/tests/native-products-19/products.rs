use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::queries::{
    NativeProducts,
    arena::{ArenaQueryCounts, EquationProduct},
    declarations::NativeDeclarations,
    demand::DemandInstances,
    graph::{ArenaNode, settle_arena_queries},
    products::{DemandProducts, EmissionOptions},
};
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn actual_native_products_emit_both_backends_and_observe_static_edits_without_value_rebuilds() {
    with_compiler_stack(|| run_product_case(false));
}

#[test]
fn aggregate_constant_slots_reach_both_emitters_and_linked_observations() {
    with_compiler_stack(|| run_product_case(true));
}

fn run_product_case(aggregate: bool) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store::DirStore::new(directory.path().join("products")));
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let options = native::Options::new(target.clone());
    let tools =
        native::Toolchain::in_tree_diagnostic(&directory.path().join("runtime"), &[&target])
            .unwrap();
    let policy = ContentHash::of_bytes(b"actual NativeProducts emitter integration");
    let mut previous: Option<(
        store::Reference,
        store::Reference,
        store::Reference,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
    )> = None;
    for (literal, fresh) in [(7u64, false), (7, true), (8, false), (8, true)] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let source = if aggregate {
            format!(
                "type Number {{n:u64}}\nconst SECOND: Number = comptime Number(n:{literal}u64)\npub fn main()->u64 {{let s=SECOND;s.n}}"
            )
        } else {
            format!("const SECOND: u64 = comptime {literal}u64\npub fn main()->u64 {{SECOND}}")
        };
        let file = engine.sources.add("products-demand.hir", source).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        if fresh {
            engine.adapters.lowering = None;
        }
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let mut declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        if fresh {
            assert_eq!(source_queries.counts.body_demands, 0);
        }
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let mut instances = DemandInstances::new(&engine, store.clone(), target.clone(), policy);
        let mut pending = instances.roots(&engine, &declarations).unwrap();
        let mut seen = BTreeSet::new();
        let mut completed = Vec::new();
        while let Some(key) = pending.pop() {
            if !seen.insert(key.instance.encoding().to_vec()) {
                continue;
            }
            let output = instances
                .instance(&mut engine, &mut declarations, &key, &options)
                .unwrap();
            pending.extend(output.dependencies.iter().cloned());
            completed.push((key, output));
        }
        completed.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        let nodes = completed
            .iter()
            .map(|(key, output)| ArenaNode {
                equations: EquationProduct {
                    owner: key.clone(),
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
        let operations = declarations.program.operation_registry();
        let manifest = hirsel_core_contract::Manifest::skeleton(
            declarations.program.operations.values().cloned(),
        );
        let environment = hirsel_lir::validate::NativeEnvironment {
            target: &target,
            operations: &operations,
            manifest: &manifest,
            discharge: Some(&options.discharge),
        };
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
        let before_create = trace.totals();
        let mut products = DemandProducts::new(
            store.clone(),
            &instances,
            &arena,
            &options,
            &emission,
            environment,
        )
        .unwrap();
        assert_eq!(
            trace.totals(),
            before_create,
            "provider creation must not execute dependencies"
        );
        let key = &completed
            .iter()
            .find(|(key, _)| key.instance.definition() == &entry_path(&default_context()))
            .unwrap()
            .0;
        let base = products.instance(key).unwrap();
        let ready = products.ready(key).unwrap();
        if fresh {
            assert_eq!(
                (
                    instances.imports,
                    instances.recipe_decodes,
                    products.local.queries.counts.transformations
                ),
                (0, 0, 0)
            );
            assert_eq!(
                trace.totals(),
                hirsel_lir::construction::ValueCounts::default()
            );
        }
        let before_emission = trace.totals();
        let (llvm, cranelift, glue_llvm, glue_cranelift, call) = products
            .with_ready(&[ready.clone()], |units, inputs| {
                assert_eq!(units.len(), 1);
                assert_eq!(inputs.statics.len(), 1);
                assert_eq!(inputs.statics[0].blocks[0].bytes, literal.to_le_bytes());
                let slots = hirsel_lir::statics::constant_slots(units[0].unit(), inputs.statics);
                assert_eq!(
                    slots.iter().any(Option::is_some),
                    aggregate,
                    "the fixture must exercise the intended constant-slot selection"
                );
                let profile = hirsel_lir::debug::Profile {
                    debug: true,
                    sources: None,
                };
                let llvm = hirsel_llvm::emit_object_profile(units, inputs, profile)
                    .map_err(|e| e.to_string())?;
                let cranelift = hirsel_cranelift::emit_object_profile(units, inputs, profile)
                    .map_err(|e| e.to_string())?;
                let finalizers = Default::default();
                let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                    inputs.layouts,
                    &finalizers,
                );
                let glue = hirsel_lir::glue::UnitGlue::of(
                    units[0].unit(),
                    inputs.layouts,
                    inputs.keys,
                    inputs.manifest,
                )?;
                let failures = hirsel_lir::glue::failure_codes(&glue.failure_rules);
                let glue_inputs = hirsel_lir::emission::GlueInputs {
                    target: inputs.target,
                    layouts: inputs.layouts,
                    keys: inputs.keys,
                    registry,
                    finalizers: &finalizers,
                    glue: &glue,
                    failures: &failures,
                    statics: inputs.statics,
                    callbacks: &[],
                    foreign: inputs.foreign,
                    aliasing_unstated: inputs.callbacks,
                };
                let glue_llvm = hirsel_llvm::emit_glue(&glue_inputs).map_err(|e| e.to_string())?;
                let glue_cranelift =
                    hirsel_cranelift::emit_glue(&glue_inputs).map_err(|e| e.to_string())?;
                let unit = units[0].unit();
                let call = native::c_call(
                    inputs.layouts,
                    &inputs.layouts.fn_abis[unit.header.abi.0 as usize],
                    &hirsel_lir::emission::symbol_name(
                        &unit.header.definition,
                        Some(&unit.header.instance),
                    ),
                    "call_main",
                );
                Ok((llvm, cranelift, glue_llvm, glue_cranelift, call))
            })
            .unwrap();
        assert_eq!(
            trace.totals(),
            before_emission,
            "emission may decode Ready but may not reconstruct a Value"
        );
        assert!(!llvm.bytes.is_empty() && !cranelift.bytes.is_empty());
        for (backend, object, glue) in [
            ("llvm", &llvm, &glue_llvm),
            ("cranelift", &cranelift, &glue_cranelift),
        ] {
            let dir = directory
                .path()
                .join(format!("{literal}-{fresh}-{backend}"));
            std::fs::create_dir_all(&dir).unwrap();
            let harness = dir.join("call.c");
            std::fs::write(&harness, include_str!("call.c").replace("@CALL@", &call)).unwrap();
            let unit = dir.join("unit.o");
            let glue_path = dir.join("glue.o");
            std::fs::write(&unit, &object.bytes).unwrap();
            std::fs::write(&glue_path, &glue.bytes).unwrap();
            let executable = dir.join("program");
            let linked = std::process::Command::new(&tools.clang)
                .arg(format!("--ld-path={}", tools.lld.display()))
                .arg(&harness)
                .arg("-I")
                .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include"))
                .arg(&unit)
                .arg(&glue_path)
                .arg(tools.archive(&target.triple).unwrap())
                .args([
                    "-lgcc_s",
                    "-lutil",
                    "-lrt",
                    "-lpthread",
                    "-lm",
                    "-ldl",
                    "-o",
                ])
                .arg(&executable)
                .output()
                .unwrap();
            assert!(
                linked.status.success(),
                "{backend} link: {}",
                String::from_utf8_lossy(&linked.stderr)
            );
            let observed = std::process::Command::new(executable).output().unwrap();
            assert!(
                observed.status.success(),
                "{backend} run: {}",
                String::from_utf8_lossy(&observed.stderr)
            );
            assert_eq!(
                observed.stdout,
                format!("{literal}\n").as_bytes(),
                "{backend} observable source result"
            );
        }
        if let Some((
            old_value,
            old_ready,
            old_context,
            old_llvm,
            old_cranelift,
            old_glue_llvm,
            old_glue_cranelift,
        )) = &previous
        {
            assert_eq!(
                base.value, *old_value,
                "constant edit keeps the caller Value equal"
            );
            assert_eq!(
                ready.ready, *old_ready,
                "constant edit keeps the Ready body equal"
            );
            if fresh {
                assert_eq!(ready.emission_context, *old_context);
                assert_eq!(llvm.bytes, *old_llvm);
                assert_eq!(cranelift.bytes, *old_cranelift);
                assert_eq!(glue_llvm.bytes, *old_glue_llvm);
                assert_eq!(glue_cranelift.bytes, *old_glue_cranelift);
            } else {
                assert_ne!(ready.emission_context, *old_context);
                if aggregate {
                    assert_ne!(
                        llvm.bytes, *old_llvm,
                        "LLVM must observe changed constant slots"
                    );
                    assert_ne!(
                        cranelift.bytes, *old_cranelift,
                        "Cranelift must observe changed constant slots"
                    );
                } else {
                    // Scalar loads relocate through the unchanged symbol;
                    // the actual linked value changes in the glue object.
                    assert_eq!(llvm.bytes, *old_llvm);
                    assert_eq!(cranelift.bytes, *old_cranelift);
                }
                assert_ne!(glue_llvm.bytes, *old_glue_llvm);
                assert_ne!(glue_cranelift.bytes, *old_glue_cranelift);
            }
        }
        previous = Some((
            base.value,
            ready.ready,
            ready.emission_context,
            llvm.bytes,
            cranelift.bytes,
            glue_llvm.bytes,
            glue_cranelift.bytes,
        ));
    }
}
