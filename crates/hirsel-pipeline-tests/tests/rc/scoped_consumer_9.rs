use crate::rc_support::*;
use hirsel_rc::import::{CachedImport, ImportCache, Lowered};
use hirsel_rc::instance::{InstanceId, Instances, Program};
struct Heap;
impl ImportCache for Heap {
    fn enabled(&self) -> bool {
        false
    }
    fn scoped_specialization(&self) -> bool {
        false
    }
    fn get(&mut self, _: &Program, _: &Instances, _: InstanceId) -> Option<CachedImport> {
        None
    }
    fn put(&mut self, _: InstanceId, _: Result<CachedImport, String>) {}
}
pub(crate) fn lower_disabled(source: &str) -> (Program, Lowered) {
    let p = compile(source);
    let l = hirsel_rc::import::lower_cached(
        &p,
        &target(),
        &Default::default(),
        &Default::default(),
        &mut Heap,
    )
    .unwrap();
    assert!(violations(&p, &l).is_empty());
    (p, l)
}
fn run(source: &str, enabled: bool) -> hirsel_heapcheck::Run {
    let (p, l) = if enabled {
        lower_valid(source)
    } else {
        lower_disabled(source)
    };
    let rows = manifest(&p);
    let target = target();
    let operations = l.operation_registry(&p);
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &l.table,
        manifest: &rows,
        operations: &operations,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::qualify_scoped_transport(&l.units, &inputs).unwrap();
    let units = l
        .units
        .iter()
        .zip(&l.unit_origins)
        .map(|(u, &origin)| {
            let placed = hirsel_rc::placement::place(
                u.clone(),
                &l.table,
                &hirsel_rc::placement::Options::for_instance(true, &l.instances, origin),
            )
            .unwrap();
            let rc = hirsel_rc::insert::insert(
                placed,
                &l.table,
                &hirsel_rc::insert::Proofs::for_instance(&l.instances, origin),
            )
            .unwrap();
            let errors = hirsel_lir::validate::check_scoped_transport(&rc, &inputs);
            assert!(errors.is_empty(), "{errors:?}\n{}", dump(&rc));
            rc
        })
        .collect();
    let h = hirsel_heapcheck::Program {
        invocation_bodies: operations.invocation_bodies.clone(),
        invocations: operations.invocations.clone(),
        pretransfer_adapters: operations.pretransfer_adapters.clone(),
        target,
        layouts: l.table,
        units,
        statics: l.statics,
    };
    let campaign = hirsel_heapcheck::fault_campaign(
        &h,
        &path("main"),
        &[],
        &mut || Box::new(hirsel_heapcheck::NoOperations),
        &Default::default(),
    );
    assert!(
        campaign.first_violation().is_none(),
        "{:?}",
        campaign.first_violation()
    );
    eprintln!(
        "enabled={enabled} counters={:?} injected={} units={}\n{source}",
        campaign.clean.counters,
        campaign.injected.len(),
        h.units.len()
    );
    campaign.clean
}
#[test]
fn scalar_site_zero_environment_control() {
    let source = "fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\nfn main()->u64{let n=3u64;invoke(fn()->u64 uses {}{n})}";
    let before = run(source, false);
    let after = run(source, true);
    assert_eq!(
        before.result.as_ref().unwrap().observation,
        after.result.as_ref().unwrap().observation
    );
    assert!(matches!(
        after.result.as_ref().unwrap().observation.returned,
        Some(hirsel_heapcheck::Observed::Integer { bits: 3, .. })
    ));
    assert_eq!(before.counters.allocations, 1);
    assert_eq!(before.counters.frees, 1);
    assert_eq!(before.counters.releases, 1);
    assert_eq!(after.counters.allocations, 0);
    assert_eq!(after.counters.frees, 0);
    assert_eq!(after.counters.releases, 0);
    assert_eq!(after.counters.dups, 0);
}

#[test]
fn empty_nested_repeated_and_owner_vectors() {
    for (body, before_allocs, after_allocs) in [
        ("invoke(fn()->u64 uses {}{3u64})", 0, 0),
        (
            "let n=3u64;invoke(fn()->u64 uses {}{invoke(fn()->u64 uses {}{n})})",
            2,
            0,
        ),
        (
            "let n=3u64;let mut total=0u64;for i in 0u64..20u64{total+=invoke(fn()->u64 uses {}{n})};total",
            20,
            0,
        ),
        (
            "let owner:box Owner=box Owner(n:3u64);invoke(fn()->u64 uses {}{owner.n})",
            2,
            1,
        ),
    ] {
        let s = format!(
            "type Owner{{n:u64}}\nfn invoke(f:fn()->u64 uses {{}})->u64 uses {{f}}{{f()}}\nfn main()->u64{{{body}}}"
        );
        let before = run(&s, false);
        let after = run(&s, true);
        assert_eq!(
            before.result.as_ref().unwrap().observation,
            after.result.as_ref().unwrap().observation
        );
        assert_eq!(before.counters.allocations, before_allocs);
        assert_eq!(after.counters.allocations, after_allocs, "{body}");
        assert_eq!(after.counters.frees, after_allocs);
    }
}
#[test]
fn recursive_forwarding_and_reentry_keep_each_activation_data() {
    let s = "fn relay(n:u64,f:fn()->u64 uses {})->u64 uses {f}{if n==0u64{f()}else{relay(n-1u64,f)}}\nfn main()->u64{let n=3u64;relay(4u64,fn()->u64 uses {}{n})}";
    let before = run(s, false);
    let after = run(s, true);
    assert_eq!(
        before.result.as_ref().unwrap().observation,
        after.result.as_ref().unwrap().observation
    );
    assert_eq!(after.counters.allocations, 0);
}
#[test]
fn resource_and_later_argument_failure_preserve_cleanup() {
    for d in [0, 1] {
        let s = format!(
            "resource type R{{n:u64}} drop(x){{let _=x.n}}\nfn invoke(f:fn()->u64 uses {{}},n:u64)->u64 uses {{f}}{{f()+n}}\nfn main()->u64{{let r=R(n:3u64);let d={d}u64;invoke(fn()->u64 uses {{}}{{r.n}},1u64/d)}}"
        );
        let before = run(&s, false);
        let after = run(&s, true);
        assert_eq!(
            before.result.as_ref().unwrap().observation,
            after.result.as_ref().unwrap().observation
        );
        assert_eq!(after.counters.finalizers, 1);
        assert_eq!(after.counters.allocations, 0);
    }
}
#[test]
fn nested_source_entry_contracts_and_mutations() {
    let s = "fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\nfn relay(g:fn(f:fn()->u64 uses {})->u64 uses {f},f:fn()->u64 uses {})->u64 uses {g,f}{g(f)}\nfn main()->u64{let n=3u64;relay(invoke,fn()->u64 uses {}{n})}";
    let (p, base) = lower_disabled(s);
    let rows = manifest(&p);
    let target = target();
    let ops = base.operation_registry(&p);
    let check = |units: &[_], table: &hirsel_lir::LayoutTable| {
        hirsel_lir::validate::qualify_scoped_transport(
            units,
            &hirsel_lir::validate::Inputs {
                target: &target,
                layouts: table,
                manifest: &rows,
                operations: &ops,
                discharge: None,
                memo: Default::default(),
            },
        )
    };
    check(&base.units, &base.table).unwrap();
    let before = run(s, false);
    let after = run(s, true);
    assert_eq!(
        before.result.as_ref().unwrap().observation,
        after.result.as_ref().unwrap().observation
    );
    for mutation in 0..3 {
        let mut table = base.table.clone();
        for u in base
            .units
            .iter()
            .filter(|u| u.header.definition.0.name == "relay")
        {
            let abi = &mut table.fn_abis[u.header.abi.0 as usize];
            match mutation {
                0 => abi.scoped_entries.clear(),
                1 => {
                    abi.scoped_entries[0].identity =
                        hirsel_syntax::ContentHash::of_bytes(b"forged-retainer")
                }
                _ => abi.scoped_entries[0].identity = abi.scoped_entries[1].identity,
            }
        }
        assert!(check(&base.units, &table).is_err(), "mutation {mutation}");
    }
}
#[test]
fn branch_selected_equal_code_keeps_distinct_captured_data() {
    for yes in [false, true] {
        let s = format!(
            "fn make(n:u64)->fn()->u64 uses {{}} {{fn()->u64 uses {{}}{{n}}}}\nfn main()->u64{{let a=make(3u64);let b=make(9u64);let f=if {yes}{{take a}}else{{take b}};f()}}"
        );
        // The owning selected callable is deliberately outside strict placement.
        let (p, l) = lower_valid(&s);
        let (p0, l0) = lower_disabled(&s);
        assert_eq!(l.units.len(), l0.units.len());
        assert!(violations(&p, &l).is_empty());
        assert!(violations(&p0, &l0).is_empty());
        let before = run(&s, false);
        let after = run(&s, true);
        assert_eq!(
            before.result.as_ref().unwrap().observation,
            after.result.as_ref().unwrap().observation
        );
        assert!(
            matches!(after.result.as_ref().unwrap().observation.returned,Some(hirsel_heapcheck::Observed::Integer{bits,..}) if bits==if yes{3}else{9})
        );
    }
}
#[test]
fn saturation_keeps_complete_sites_at_the_heap_baseline() {
    let mut s = String::from(
        "fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\nfn main()->u64{let n=3u64;let mut sum=0u64;",
    );
    for i in 0..20 {
        s.push_str(&format!("sum+=invoke(fn()->u64 uses {{}}{{n+{i}u64}});"));
    }
    s.push_str("sum}");
    let before = run(&s, false);
    let after = run(&s, true);
    assert_eq!(
        before.result.as_ref().unwrap().observation,
        after.result.as_ref().unwrap().observation
    );
    assert_eq!(before.counters.allocations, 20);
    assert_eq!(after.counters.allocations, 20);
    let (_, l) = lower_valid(&s);
    assert!(l.units.len() - l.instances.len() <= hirsel_rc::closure_calls::MAX_SPECIALIZATIONS);
    let mut count = std::collections::BTreeMap::new();
    for &o in &l.unit_origins {
        *count.entry(o.0).or_insert(0usize) += 1;
    }
    assert!(
        count
            .values()
            .all(|&n| n <= hirsel_rc::closure_calls::MAX_PER_FUNCTION + 1)
    );
}
#[test]
fn native_zero_environment_on_both_backends() {
    use hirsel_driver::native::*;
    use sha2::{Digest, Sha256};
    let source="fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\nfn value(n:u64)->u64{invoke(fn()->u64 uses {}{invoke(fn()->u64 uses {}{n})})}\npub fn main(rt:Io,args:Args)->Result[(),u64]{Error(value(3u64))}".to_string();
    let target = target();
    let root =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("scoped-consumer-9-native");
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
        let h = heap_campaign(&ready, &entry, &sites).unwrap();
        assert_eq!(h.observation, reference);
        assert_eq!(h.counters.allocations, 0);
        assert_eq!(h.counters.frees, 0);
        assert_eq!(h.counters.releases, 0);
        eprintln!(
            "native zero-environment H {:?}, specializations {}",
            h.counters,
            ready.inline_instances.len()
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
fn shared_capture_preserves_its_owner_ledger() {
    let s = "fn invoke(f:fn()->bool uses {})->bool uses {f}{f()}\nfn main()->bool{let built:String=\"value\";let s:Shared[String]=shared.freeze(take built);invoke(fn()->bool uses {}{let t=copy s;match shared.thaw(take t){Ok(text)=>true,Error(root)=>false}})}";
    let before = run(s, false);
    let after = run(s, true);
    assert_eq!(
        before.result.as_ref().unwrap().observation,
        after.result.as_ref().unwrap().observation
    );
    assert_eq!(after.counters.allocations + 1, before.counters.allocations);
    assert_eq!(after.counters.frees, after.counters.allocations);
    assert_eq!(after.counters.finalizers, before.counters.finalizers);
}

#[test]
fn enabled_erased_apply_uses_direct_capture_targets_and_exact_data() {
    let s = "fn apply(x:u64,f:fn(u64)->u64 uses {})->u64 uses {f}{f(x)}\nfn main()->u64{let a=3u64;let b=2u64;let add=fn(x:u64)->u64 uses {}{x+a};let mul=fn(x:u64)->u64 uses {}{x*b};apply(apply(4u64,add),mul)}";
    let before = run(s, false);
    let after = run(s, true);
    assert_eq!(
        before.result.as_ref().unwrap().observation,
        after.result.as_ref().unwrap().observation
    );
    assert!(matches!(
        after.result.as_ref().unwrap().observation.returned,
        Some(hirsel_heapcheck::Observed::Integer { bits: 14, .. })
    ));
    assert_eq!(after.counters.allocations, 0);
    assert_eq!(after.counters.releases, 0);
    let (_, l) = lower_valid(s);
    let main = l
        .units
        .iter()
        .find(|u| u.header.definition.0.name == "main" && u.parameters.is_empty())
        .unwrap();
    for call in main
        .tables
        .calls
        .iter()
        .filter(|c| matches!(c.callee, hirsel_lir::ir::Callee::Direct(_)))
    {
        let hirsel_lir::ir::Callee::Direct(symbol) = call.callee else {
            unreachable!()
        };
        let target = &main.tables.symbols[symbol.index()];
        if target.definition.0.name != "apply" {
            continue;
        }
        let body = l
            .units
            .iter()
            .find(|u| Some(u.header.instance) == target.instance)
            .unwrap();
        assert!(
            body.tables
                .calls
                .iter()
                .all(|c| matches!(c.callee, hirsel_lir::ir::Callee::Direct(_)))
        );
    }
}

#[test]
fn native_counted_owner_capture_preserves_one_owner() {
    use hirsel_driver::native::*;
    use sha2::{Digest, Sha256};
    let source="type Owner{n:u64}\nfn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\nfn value(n:u64)->u64{let owner:box Owner=box Owner(n:n);invoke(fn()->u64 uses {}{owner.n})}\npub fn main(rt:Io,args:Args)->Result[(),u64]{Error(value(3u64))}".to_string();
    let target = target();
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("scoped-consumer-9-native-owner");
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
        let h = heap_campaign(&ready, &entry, &sites).unwrap();
        assert_eq!(h.observation, reference);
        assert_eq!(h.counters.allocations, 1);
        assert_eq!(h.counters.frees, 1);
        assert_eq!(h.counters.releases, 1);
        eprintln!(
            "native zero-environment H {:?}, specializations {}",
            h.counters,
            ready.inline_instances.len()
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
fn structural_rejection_keeps_exact_heap_lowering() {
    for source in [
        "fn invoke(f:fn()->String uses {})->String uses {f}{f()}\nfn main()->String{let n=3u64;invoke(fn()->String uses {}{if n==3u64{\"yes\"}else{\"no\"}})}",
        "fn main()->u64{let n=3u64;let a=fn()->u64 uses {}{n};let b=fn()->u64 uses {}{n+1u64};let f=if true{take a}else{take b};f()}",
        "type Owner{n:u64}\nfn invoke(take f:once fn()->u64)->u64 uses {f}{f()}\nfn main()->u64{let n:box Owner=box Owner(n:3u64);let f:once fn()->u64=fn()->u64{let owner=take n;owner.n};invoke(take f)}",
    ] {
        let (_, before) = lower_disabled(source);
        let (_, after) = lower_valid(source);
        let contents = |l: &Lowered| l.units.iter().map(|u| u.digest()).collect::<Vec<_>>();
        assert_eq!(contents(&before), contents(&after), "{source}");
        assert_eq!(before.table.digest(), after.table.digest(), "{source}");
    }
}

#[test]
fn zero_size_sites_keep_exact_existing_lowering() {
    for source in [
        "fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\nfn main()->u64{invoke(fn()->u64 uses {}{3u64})}",
        "fn check(ok:bool)->() requires ok {}\nfn main()->u64{check(true);3u64}",
    ] {
        let (_, before) = lower_disabled(source);
        let (_, after) = lower_valid(source);
        assert_eq!(
            before.units.iter().map(|u| u.digest()).collect::<Vec<_>>(),
            after.units.iter().map(|u| u.digest()).collect::<Vec<_>>(),
            "{source}"
        );
        assert_eq!(before.table.digest(), after.table.digest(), "{source}");
        let observed = run(source, true);
        assert_eq!(observed.counters.allocations, 0);
        assert_eq!(observed.counters.frees, 0);
        assert_eq!(observed.counters.releases, 0);
    }
}

#[test]
fn nested_duplicate_specializations_keep_their_callable_proof_owner() {
    for (source, noalloc) in [
        (
            "fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\n\
             fn main()->u64{let n=3u64;let f=fn()->u64 uses {}{invoke(fn()->u64 uses {}{n})};invoke(f)+invoke(f)}",
            false,
        ),
        (
            "@noalloc fn invoke(f:@noalloc fn()->u64 uses {})->u64 uses {f}{f()}\n\
             @noalloc fn value(n:u64)->u64{invoke(fn()->u64 uses {}{n})}\n\
             fn main()->u64{value(6u64)}",
            true,
        ),
    ] {
        let (_, lowered) = lower_valid(source);
        assert!(lowered.units.len() > lowered.instances.len());
        assert_eq!(lowered.units.len(), lowered.unit_origins.len());
        let mut specialized_invoke = 0;
        let mut identities = std::collections::BTreeSet::new();
        for (i, (unit, &origin)) in lowered.units.iter().zip(&lowered.unit_origins).enumerate() {
            assert!(identities.insert((unit.header.definition.clone(), unit.header.instance)));
            assert_eq!(
                unit.header.definition,
                *lowered.instances.view(origin).key.definition()
            );
            if unit.header.definition.0.name == "invoke" {
                assert_eq!(
                    hirsel_rc::placement::noalloc(&lowered.instances, origin),
                    noalloc
                );
                assert_eq!(
                    hirsel_rc::insert::Proofs::for_instance(&lowered.instances, origin).noalloc,
                    noalloc
                );
                specialized_invoke += usize::from(i >= lowered.instances.len());
            }
        }
        assert!(specialized_invoke > 0);
        if !noalloc {
            let main = lowered
                .units
                .iter()
                .find(|u| u.header.definition.0.name == "main")
                .unwrap();
            let targets: Vec<_> = main
                .tables
                .calls
                .iter()
                .filter_map(|call| {
                    let hirsel_lir::ir::Callee::Direct(symbol) = call.callee else {
                        return None;
                    };
                    let symbol = &main.tables.symbols[symbol.index()];
                    (symbol.definition.0.name == "invoke").then_some(symbol.instance)
                })
                .collect();
            assert_eq!(targets.len(), 2);
            assert_eq!(
                targets[0], targets[1],
                "duplicate sites reuse their content-named body"
            );
        }
        let after = run(source, true);
        let audit = after.result.unwrap();
        assert!(matches!(
            audit.observation.returned,
            Some(hirsel_heapcheck::Observed::Integer { bits: 6, .. })
        ));
    }
}

#[test]
fn specialization_cannot_grant_noalloc_to_an_allocating_callback() {
    let rejected = try_compile(
        "@noalloc fn invoke(f:@noalloc fn()->u64 uses {})->u64 uses {f}{f()}\n\
         fn allocating()->u64{3u64}\n\
         fn main()->u64{invoke(allocating)}",
    )
    .err()
    .expect("an allocating declaration cannot satisfy a noalloc callback contract");
    assert!(rejected.contains("MEM-5"), "{rejected}");
}
