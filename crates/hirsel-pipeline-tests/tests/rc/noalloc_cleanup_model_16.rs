//! Accepted instantiations execute against an ownership/exit oracle, then fault injection.
use crate::rc_support::*;
use hirsel_heapcheck::{Config, Event, NoOperations, Observed};
use hirsel_lir::{ir::LirUnit, stage::Ready, validate::Inputs};
use hirsel_rc::{cleanup::Finalizers, insert::Proofs, placement::Options};

fn program(source: &str) -> hirsel_heapcheck::Program<Ready> {
    let p = compile(source); // J and independent TIR validation on export/import.
    let lowered = hirsel_rc::import::lower(&p, &target()).unwrap();
    let mut rc = vec![];
    for (unit, &instance) in lowered.units.iter().zip(&lowered.unit_origins) {
        let placed = hirsel_rc::placement::place(
            unit.clone(),
            &lowered.table,
            &Options::for_instance(true, &lowered.instances, instance),
        )
        .unwrap();
        rc.push(
            hirsel_rc::insert::insert(
                placed,
                &lowered.table,
                &Proofs::for_instance(&lowered.instances, instance),
            )
            .unwrap(),
        );
    }
    let finalizers = Finalizers::new(&lowered.instances, &rc);
    let units: Vec<LirUnit<Ready>> = rc
        .into_iter()
        .map(|u| hirsel_rc::cleanup::lower(u, &lowered.table, &finalizers).unwrap())
        .collect();
    let rows = manifest(&p);
    let target = target();
    let operations = p.operation_registry();
    let inputs = Inputs {
        target: &target,
        layouts: &lowered.table,
        manifest: &rows,
        operations: &operations,
        memo: Default::default(),
        discharge: None,
    };
    for unit in &units {
        let errors = hirsel_lir::validate::check(unit, &inputs);
        assert!(errors.is_empty(), "{errors:?}\n{}", dump(unit));
    }
    hirsel_heapcheck::Program {
        target,
        layouts: lowered.table,
        units,
        statics: lowered.statics,
    }
}

#[derive(Clone, Copy, Debug)]
enum Exit {
    Return,
    Argument,
    Requires,
    Ensures,
}

/// DROP-2 and MEM-5: `take` moves a binding into a caller-owned temporary;
/// entry then transfers all temporaries together. Failure before entry drops
/// the temporary stack first, then the remaining original bindings.
fn drop_order(taken: bool, reverse: bool, exit: Exit) -> Vec<u32> {
    let mut bindings = vec![0, 1];
    if !taken || matches!(exit, Exit::Requires) {
        return bindings.into_iter().rev().collect();
    }
    let operands = if reverse {
        vec![Some(1), Some(0), None]
    } else {
        vec![Some(0), None, Some(1)]
    };
    let mut temporaries = vec![];
    for operand in operands {
        match operand {
            Some(value) => {
                bindings.retain(|v| *v != value);
                temporaries.push(value);
            }
            None if matches!(exit, Exit::Argument) => {
                return temporaries
                    .into_iter()
                    .rev()
                    .chain(bindings.into_iter().rev())
                    .collect();
            }
            None => (),
        }
    }
    // Successful entry and return preserve operand order in the result tuple.
    assert!(bindings.is_empty());
    temporaries.into_iter().rev().collect()
}

fn exercise(source: &str, order: &[u32], expected_failures: &[&str]) -> usize {
    let program = program(source);
    let campaign = hirsel_heapcheck::fault_campaign(
        &program,
        &path("main"),
        &[],
        &mut || Box::new(NoOperations),
        &Config {
            full_recount_every_step: true,
            ..Config::default()
        },
    );
    assert!(
        campaign.first_violation().is_none(),
        "{source}\n{:?}",
        campaign.first_violation()
    );
    let clean = &campaign.clean;
    let audit = clean.result.as_ref().unwrap();
    assert_eq!(clean.counters.allocations, 0, "{source}");
    assert_eq!(clean.counters.live_heap_bytes, 0);
    assert_eq!(clean.counters.finalizers, order.len() as u64);
    let dropped: Vec<_> = clean
        .trace
        .iter()
        .filter_map(|e| match e {
            Event::Finalize { resource } => Some(resource.0),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, order, "{source}");
    let failures: Vec<_> = audit
        .observation
        .failures
        .iter()
        .map(|f| f.rule.as_str())
        .collect();
    assert_eq!(failures, expected_failures, "{source}");
    if let Some(primary) = audit.observation.failures.first() {
        assert_eq!(primary.definition.0.name.as_str(), "caller");
        let base = item_bases(source)[&primary.definition] as usize;
        let site = &source[base + primary.site.0 as usize..base + primary.site.1 as usize];
        assert!(
            matches!(site, "10u64/z" | "10u64/d" | "flag"),
            "primary site: {site}"
        );
    }
    if expected_failures.is_empty() {
        assert!(matches!(
            audit.observation.returned,
            Some(Observed::Integer { bits: 7, .. })
        ));
    }
    for (site, run) in &campaign.injected {
        let audit = run.result.as_ref().unwrap();
        assert_eq!(run.counters.allocations, 0);
        assert_eq!(run.counters.live_heap_bytes, 0);
        // In the failing-finalizer witness, site zero is the original
        // argument division. Later injected sites occur during its cleanup:
        // FAIL-3 keeps that already-raised failure primary.
        if expected_failures.len() > 1 && *site > 0 {
            assert_eq!(
                audit.observation.failures[0],
                clean.result.as_ref().unwrap().observation.failures[0]
            );
        } else {
            assert_eq!(
                audit.observation.failures[0].rule,
                hirsel_heapcheck::INJECTED_RULE
            );
        }
        assert!(
            audit
                .observation
                .failures
                .iter()
                .any(|failure| failure.rule == hirsel_heapcheck::INJECTED_RULE)
        );
    }
    assert!(!campaign.injected.is_empty());
    campaign.injected.len() + 1
}

#[test]
fn two_resources_and_a_fallible_later_argument() {
    let mut count = 0;
    let mut runs = 0;
    for taken in [false, true] {
        for reverse in [false, true] {
            for exit in [Exit::Return, Exit::Argument, Exit::Requires, Exit::Ensures] {
                let mode = if taken { "take " } else { "" };
                let proof = if taken { "T:noalloc_drop" } else { "T" };
                let result = if taken { "(T,T)" } else { "u64" };
                let body = if taken {
                    "(take first,take second)"
                } else {
                    "n"
                };
                let (params, args) = if reverse {
                    (
                        format!("{mode}first:T,{mode}second:T,n:u64"),
                        format!("{mode}b,{mode}a,10u64/z"),
                    )
                } else {
                    (
                        format!("{mode}first:T,n:u64,{mode}second:T"),
                        format!("{mode}a,10u64/z,{mode}b"),
                    )
                };
                let contract = match exit {
                    Exit::Requires => "requires flag",
                    Exit::Ensures => "ensures flag",
                    _ => "",
                };
                let z = if matches!(exit, Exit::Argument) { 0 } else { 1 };
                let flag = !matches!(exit, Exit::Requires | Exit::Ensures);
                let source = format!(
                    "resource type R {{n:u64}} @noalloc drop(x) {{()}}\n@noalloc fn relay[{proof}]({params})->{result} {{{body}}}\n@noalloc fn caller[{proof}]({mode}a:T,{mode}b:T,z:u64,flag:bool)->{result} {contract} {{relay({args})}}\nfn main()->u64 {{let a=((R(n:1u64),),);let b=((R(n:2u64),),);let _=caller({mode}a,{mode}b,{z}u64,{flag});7u64}}"
                );
                let failure = match exit {
                    Exit::Return => None,
                    Exit::Argument => Some("SEM-1"),
                    _ => Some("FACT-1"),
                };
                runs += exercise(
                    &source,
                    &drop_order(taken, reverse, exit),
                    &failure.into_iter().collect::<Vec<_>>(),
                );
                count += 1;
            }
        }
    }
    assert_eq!(count, 16);
    eprintln!(
        "noalloc_cleanup_model_16: {count} named resource cases, {runs} checked-heap executions"
    );
}

#[test]
fn one_resource_capture_has_noalloc_invocation_and_cleanup() {
    let mut runs = 0;
    for fail in [false, true] {
        // Erasure through a borrowed signature promises invocation only. The
        // resource remains caller-owned and is dropped after the borrowed environment.
        let d = if fail { 0 } else { 1 };
        let source = format!(
            "resource type R {{n:u64}} @noalloc drop(x) {{()}}\n@noalloc fn invoke(f:@noalloc fn()->u64 uses {{}},n:u64)->u64 uses {{f}} {{f()+n}}\n@noalloc fn caller(d:u64)->u64 {{let r=R(n:7u64);let f=fn()->u64 uses {{}} {{r.n}};invoke(f,10u64/d);7u64}}\nfn main()->u64 {{caller({d}u64)}}"
        );
        runs += exercise(&source, &[0], if fail { &["SEM-1"] } else { &[] });
    }
    eprintln!("noalloc_cleanup_model_16: 2 capture cases, {runs} checked-heap executions");
}

#[test]
fn original_argument_failure_stays_primary_when_both_drops_fail() {
    let source = "resource type R {n:u64} @noalloc drop(x) {let _=10u64/x.n}\n@noalloc fn relay[T](take a:T,n:u64,take b:T)->(T,T) {(take a,take b)}\n@noalloc fn caller[T:noalloc_drop](take a:T,take b:T,z:u64)->(T,T) {relay(take a,10u64/z,take b)}\nfn main()->u64 {let a=R(n:0u64);let b=R(n:0u64);let _=caller(take a,take b,0u64);7u64}";
    let runs = exercise(source, &[0, 1], &["SEM-1", "SEM-1", "SEM-1"]);
    eprintln!("noalloc_cleanup_model_16: 1 failing-cleanup case, {runs} checked-heap executions");
}
