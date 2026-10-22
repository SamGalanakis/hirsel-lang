//! Raw physical counterexamples paired with ordinary owning controls.
use crate::rc_support::*;
use hirsel_lir::validate::Inputs;

fn heap(source: &str) -> hirsel_heapcheck::Run {
    heap_baseline(source, true)
}
fn heap_baseline(source: &str, qualify: bool) -> hirsel_heapcheck::Run {
    let (program, lowered) = lower_valid(source);
    let rows = manifest(&program);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &program.operation_registry(),
        target: &target,
        layouts: &lowered.table,
        manifest: &rows,
    };
    if qualify {
        let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
        for (p, s) in &program.signatures {
            signatures.insert(hirsel_syntax::DefId::intern(p), (**s).clone());
        }
        for definition in program
            .definitions
            .values()
            .filter(|d| !d.bodies.is_empty())
        {
            let proof = hirsel_validate::qualify_scoped_transport(
                definition,
                &program.types,
                &signatures,
                &mut vec![],
            );
            assert!(
                proof.violations.is_empty(),
                "{}: {proof:?}",
                definition.path
            );
        }
        hirsel_lir::validate::qualify_scoped_transport(&lowered.units, &inputs).unwrap();
    }
    let units = lowered
        .units
        .iter()
        .zip(&lowered.unit_origins)
        .map(|(u, &origin)| {
            let placed = hirsel_rc::placement::place(
                u.clone(),
                &lowered.table,
                &hirsel_rc::placement::Options::for_instance(true, &lowered.instances, origin),
            )
            .unwrap();
            let rc = hirsel_rc::insert::insert(
                placed,
                &lowered.table,
                &hirsel_rc::insert::Proofs::for_instance(&lowered.instances, origin),
            )
            .unwrap();
            let errors = if qualify {
                hirsel_lir::validate::check_scoped_transport(&rc, &inputs)
            } else {
                hirsel_lir::validate::check(&rc, &inputs)
            };
            assert!(errors.is_empty(), "{errors:?}\n{}", dump(&rc));
            rc
        })
        .collect();
    let h = hirsel_heapcheck::Program {
        target,
        layouts: lowered.table,
        units,
        statics: lowered.statics,
    };
    let campaign = hirsel_heapcheck::fault_campaign(
        &h,
        &path("main"),
        &[],
        &mut || Box::new(hirsel_heapcheck::NoOperations),
        &Default::default(),
    );
    eprintln!(
        "heap baseline counters={:?}, injected={}\n{source}",
        campaign.clean.counters,
        campaign.injected.len()
    );
    assert!(
        campaign.first_violation().is_none(),
        "{:?}",
        campaign.first_violation()
    );
    campaign.clean
}

#[test]
fn source_envelope_and_heap_baseline_survive() {
    for (body, allocations) in [
        ("invoke(fn()->u64 uses {} {3u64})", 0),
        ("let n=3u64;invoke(fn()->u64 uses {} {n})", 1),
        (
            "let n=3u64;invoke(fn()->u64 uses {} {invoke(fn()->u64 uses {} {n})})",
            2,
        ),
        (
            "let owner:box Owner=box Owner(n:3u64);invoke(fn()->u64 uses {} {owner.n})",
            2,
        ),
        (
            "let n=3u64;let mut total=0u64;for i in 0u64..20u64 {total+=invoke(fn()->u64 uses {} {n})};total",
            20,
        ),
    ] {
        let run = heap(&format!(
            "type Owner {{n:u64}}\nfn invoke(f:fn()->u64 uses {{}})->u64 uses {{f}} {{f()}}\nfn main()->u64 {{{body}}}"
        ));
        assert_eq!(run.counters.allocations, allocations);
        assert_eq!(run.counters.frees, allocations);
    }
}

#[test]
fn taken_callback_return_and_scoped_source_refusal_are_preserved() {
    let rejected = try_compile("fn keep(f:fn()->u64 uses {})->fn()->u64 uses {} {f}").unwrap_err();
    assert!(rejected.contains("CLO-1"), "{rejected}");
    lower_valid(
        "fn keep(take f:fn()->u64 uses {})->fn()->u64 uses {} {take f}\nfn main()->u64 {let f:fn()->u64 uses {}=fn()->u64 uses {} {3u64};let g=keep(take f);g()}",
    );
}

#[test]
fn generic_nested_and_recursive_forwarding_keep_the_contract() {
    for source in [
        "fn relay[T](x:T,f:fn(T)->u64 uses {})->u64 uses {f} {f(x)}\nfn main()->u64 {let n=3u64;relay(4u64,fn(x:u64)->u64 uses {} {x+n})}",
        "fn invoke(f:fn()->u64 uses {})->u64 uses {f} {f()}\nfn main()->u64 {let n=3u64;invoke(fn()->u64 uses {} {invoke(fn()->u64 uses {} {n})})}",
        "fn relay(n:u64,f:fn()->u64 uses {})->u64 uses {f} {if n==0u64 {f()} else {relay(n-1u64,f)}}\nfn main()->u64 {let n=3u64;relay(4u64,fn()->u64 uses {} {n})}",
        "fn keep[T:copyable](x:T)->T in x {x}\nfn main()->u64 {let f:fn()->u64 uses {}=fn()->u64 uses {} {3u64};let g=keep(f);g()}",
    ] {
        let (program, lowered) = lower_valid(source);
        hirsel_lir::validate::check_scoped_calls(&lowered.units, &lowered.table).unwrap();
        assert!(violations(&program, &lowered).is_empty());
    }
}

#[test]
fn stripped_source_selectors_and_linked_body_mismatch_are_refused() {
    let source = "fn relay(f:fn()->u64 uses {})->u64 uses {f} {invoke(f)}\nfn invoke(f:fn()->u64 uses {})->u64 uses {f} {f()}\nfn main()->u64 {let n=3u64;invoke(fn()->u64 uses {} {n})}";
    let (program, mut lowered) = lower_valid(source);
    let original = &program.definitions[&path("invoke")];
    let mut forged = (**original).clone();
    forged.bodies[forged.entry.0 as usize]
        .scoped_parameters
        .clear();
    let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
    for (p, s) in &program.signatures {
        signatures.insert(hirsel_syntax::DefId::intern(p), (**s).clone());
    }
    let errors = hirsel_validate::validate(&forged, &program.types, &signatures, &mut vec![]);
    assert!(!errors.violations.is_empty());
    let targets: Vec<_> = lowered
        .units
        .iter()
        .enumerate()
        .filter_map(|(i, u)| {
            (u.header.definition.0.name == "invoke"
                && !lowered.table.fn_abis[u.header.abi.0 as usize]
                    .nonretaining
                    .is_empty())
            .then_some(i)
        })
        .collect();
    assert!(!targets.is_empty());
    for u in targets {
        let mut abi = lowered.table.fn_abis[lowered.units[u].header.abi.0 as usize].clone();
        abi.nonretaining.clear();
        abi.scoped_contract =
            hirsel_lir::abi::scoped_contract_digest(&[], &abi.nonretaining_captures);
        let id = hirsel_lir::FnAbiId(lowered.table.fn_abis.len() as u32);
        lowered.table.fn_abis.push(abi);
        lowered.units[u].header.abi = id;
    }
    assert!(hirsel_lir::validate::check_scoped_calls(&lowered.units, &lowered.table).is_err());
}

#[test]
fn borrowed_resource_cleanup_survives_success_and_failure() {
    for divisor in [0, 1] {
        let run = heap(&format!(
            "resource type R {{n:u64}} drop(x) {{let _=x.n}}\nfn invoke(f:fn()->u64 uses {{}})->u64 uses {{f}} {{f()}}\nfn main()->u64 {{let r=R(n:3u64);let divisor={divisor}u64;invoke(fn()->u64 uses {{}} {{r.n/divisor}})}}"
        ));
        assert_eq!(run.counters.finalizers, 1);
        assert_eq!(run.counters.allocations, 1);
        assert_eq!(run.counters.frees, 1);
    }
}

#[test]
fn heap_callable_representation_survives_both_native_backends() {
    use hirsel_driver::native::*;
    use sha2::{Digest, Sha256};
    let source="@noalloc fn invoke(f:@noalloc fn()->u64 uses {})->u64 uses {f}{f()}\n@noalloc fn value(n:u64)->u64{invoke(fn()->u64 uses {}{n})}\npub fn main(rt:Io,args:Args)->Result[(),u64]{Error(value(3u64))}".to_string();
    let target = target();
    let root =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scoped-transport-native");
    let toolchain = Toolchain::in_tree_diagnostic(&root.join("runtime"), &[&target]).unwrap();
    for (variant, archive) in &toolchain.runtime {
        eprintln!(
            "runtime {variant:?} sha256={:x}",
            Sha256::digest(std::fs::read(archive).unwrap())
        );
    }
    hirsel_engine::with_compiler_stack(move || {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine.sources.add("scoped.hir", source).unwrap();
        let checked =
            hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = hirsel_engine::entry_path(&hirsel_engine::default_context());
        let reference = hirsel_engine::run_observed_snapshot(
            &mut engine,
            &hirsel_engine::RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
        let raw = engine.export_native_input().unwrap();
        let input = engine.import_native_input(raw).unwrap();
        let sites = Sites::new(input.snapshot());
        let ready = build(&input, &Options::new(target)).unwrap();
        assert_eq!(
            heap_campaign(&ready, &entry, &sites).unwrap().observation,
            reference
        );
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = root.join(backend.name());
            std::fs::create_dir_all(&dir).unwrap();
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::Speed,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            for (i, object) in objects.iter().enumerate() {
                eprintln!(
                    "{} object {i} sha256={:x}",
                    backend.name(),
                    Sha256::digest(&object.bytes)
                );
            }
            let executable = link_objects(&ready, &entry, &objects, &toolchain, &dir).unwrap();
            eprintln!(
                "{} executable sha256={:x}",
                backend.name(),
                Sha256::digest(std::fs::read(&executable).unwrap())
            );
            let launched = launch(&executable, &ready.target, &toolchain, &dir, true).unwrap();
            assert_eq!(observe(&launched, &ready, &sites).unwrap(), reference);
        }
    });
}

#[test]
fn borrowed_shared_root_preserves_its_owner_cleanup() {
    let run = heap(
        "fn invoke(f:fn()->bool uses {})->bool uses {f} {f()}\nfn main()->bool {let built:String=\"value\";let s:Shared[String]=shared.freeze(take built);invoke(fn()->bool uses {} {let t=copy s;match shared.thaw(take t) {Ok(text)=>true,Error(root)=>false}})}",
    );
    assert_eq!(run.counters.allocations, run.counters.frees);
}

#[test]
fn unsupported_generic_borrow_preserves_execution_but_not_qualification() {
    let source = "fn discard[T](x:T) {}\nfn invoke(f:fn()->u64 uses {})->u64 uses {f} {discard(f);f()}\nfn main()->u64 {let n=3u64;invoke(fn()->u64 uses {} {n})}";
    let (program, lowered) = lower_valid(source);
    let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
    for (p, s) in &program.signatures {
        signatures.insert(hirsel_syntax::DefId::intern(p), (**s).clone());
    }
    let definition = &program.definitions[&path("invoke")];
    assert!(
        hirsel_validate::validate(definition, &program.types, &signatures, &mut vec![])
            .violations
            .is_empty()
    );
    let proof = hirsel_validate::qualify_scoped_transport(
        definition,
        &program.types,
        &signatures,
        &mut vec![],
    );
    assert!(
        proof.violations.iter().any(|v| matches!(
            v.kind,
            hirsel_validate::ViolationKind::TypeMismatch {
                entity: "scoped may-retain source call"
            }
        )),
        "{proof:?}"
    );
    let rows = manifest(&program);
    let target = target();
    let inputs = Inputs {
        target: &target,
        layouts: &lowered.table,
        manifest: &rows,
        operations: &program.operation_registry(),
        discharge: None,
        memo: Default::default(),
    };
    let proof = hirsel_lir::validate::qualify_scoped_transport(&lowered.units, &inputs);
    assert!(proof.is_err(), "generic evidence must remain unqualified");
    // Ordinary C, placement, RC and H keep the current heap representation.
    let run = heap_baseline(source, false);
    assert_eq!(run.counters.allocations, 1);
    assert_eq!(run.counters.frees, 1);
}

#[test]
fn owning_scoped_local_capture_keeps_the_heap_fallback() {
    let source = "fn invoke(f:fn()->u64 uses {})->u64 uses {f} {let g=fn()->u64 uses {} {f()};g()}\nfn main()->u64 {let n=3u64;invoke(fn()->u64 uses {} {n})}";
    let run = heap_baseline(source, false);
    assert_eq!(run.counters.allocations, run.counters.frees);
}
