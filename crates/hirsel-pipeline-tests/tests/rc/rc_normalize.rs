//! Mandatory self-tail loops: only published sites, each re-verified per
//! instance; everything else stays a call. Parameter permutations of a helper
//! update the helper and all its callers in one transform.
use crate::rc_support as support;

use hirsel_rc::import::LowerError;
use hirsel_rc::instance::Program;
use hirsel_rc::normalize::{Home, NormalizeError, PermuteError, permute_parameters};
use hirsel_tir::TailKind;
use support::*;

/// The published self-tail programs of the driver's tail-call suite.
const LOOPS: &[(&str, &str)] = &[
    (
        "fn sum(n:u64,total:u64)->u64 { if n == 0u64 { total } else { sum(n-1u64,total+n) } }\nfn main()->u64 {sum(10000u64,0u64)}",
        "sum",
    ),
    (
        "fn descend(n:u64,text:String)->u64 { if n == 0u64 { string.byte_len(text) } else { descend(n-1u64,std@1/u64.to_string(n)) } }\nfn main()->u64 {descend(1000u64,std@1/u64.to_string(7u64))}",
        "descend",
    ),
    (
        "resource type R {n:u64} drop(x) {}\nfn descend(n:u64,take r:R)->u64 { if n == 0u64 { r.n } else { descend(n-1u64,take r) } }\nfn main()->u64 {descend(1000u64,R(n:7u64))}",
        "descend",
    ),
    (
        "fn descend[n:size](left:u64,xs:Inline[u8,n])->u64 { if left == 0u64 {n} else {descend(left-1u64,xs)} }\nfn main()->u64 {let xs:Inline[u8,7]=[1u8,2u8,3u8,4u8,5u8,6u8,7u8]; descend(1000u64,xs)}",
        "descend",
    ),
    (
        "fn descend[T:plain](n:u64,x:T)->u64 {let alias=x; if n == 0u64 {7u64} else {descend(n-1u64,alias)}}\nfn main()->u64 {descend(1000u64,1u8)}",
        "descend",
    ),
    (
        "type W {n:u64}\nfn descend(n:u64)->u64 {let w=W(n:n); if n == 0u64 {7u64} else {descend(n-1u64)}}\nfn main()->u64 {descend(1000u64)}",
        "descend",
    ),
    (
        "type W {text:String}\nfn descend(n:u64,x:W)->u64 {if n == 0u64 {7u64} else {descend(n-1u64,W(text:std@1/u64.to_string(n)))}}\nfn main()->u64 {descend(1000u64,W(text:std@1/u64.to_string(1u64)))}",
        "descend",
    ),
    (
        "fn descend[T:data](n:u64,x:T)->u64 {if n == 0u64 {7u64} else {descend(n-1u64,copy x)}}\nfn main()->u64 {descend(1000u64,std@1/u64.to_string(1u64))}",
        "descend",
    ),
];

#[test]
fn every_published_self_tail_site_becomes_a_loop_with_one_frame() {
    for (source, name) in LOOPS {
        let (program, lowered) = lower(source);
        let ids = instances_of(&lowered, name);
        assert!(!ids.is_empty(), "{source}");
        for id in ids {
            let unit = &lowered.units[id.index()];
            let plan = lowered.plans[id.index()].as_ref().unwrap_or_else(|| {
                panic!("{source}\nno loop plan; declined {:?}", lowered.declined)
            });
            assert_eq!(plan.sites.len(), 1, "{source}");
            // Bounded frame depth: the unit never calls itself, it loops.
            assert_eq!(self_calls(unit), 0, "{source}\n{}", dump(unit));
            assert!(has_cycle(unit), "{source}\n{}", dump(unit));
            let v = unit_violations(&program, &lowered, id);
            assert!(v.is_empty(), "{source}\n{}\n{v:#?}", dump(unit));
        }
    }
}

#[test]
fn borrowed_counted_temporaries_ride_in_owned_loop_slots() {
    // LANG-10: the borrowed String argument is a fresh counted temporary.
    let (program, lowered) = lower(LOOPS[1].0);
    let id = instances_of(&lowered, "descend")[0];
    let plan = lowered.plans[id.index()].as_ref().unwrap();
    assert_eq!(plan.sites[0].owned.len(), 1);
    assert_eq!(plan.homes, vec![Home::Loop, Home::Loop]);
    let unit = &lowered.units[id.index()];
    let text = dump(unit);
    // The previous owner is released at the replacement and at the exit.
    let releases = unit
        .tables
        .cleanup_anchors
        .iter()
        .filter(|a| a.cleanup == hirsel_rc::import::LOOP_RELEASE)
        .count();
    assert!(releases >= 1, "{text}");
    assert!(unit_violations(&program, &lowered, id).is_empty(), "{text}");
}

#[test]
fn an_unchanged_borrowed_parameter_stays_in_the_callers_storage() {
    let source = "fn walk(n:u64,text:String)->u64 { if n == 0u64 { string.byte_len(text) } else { walk(n-1u64,text) } }\nfn main()->u64 {walk(5u64,std@1/u64.to_string(3u64))}";
    let (program, lowered) = lower(source);
    let id = instances_of(&lowered, "walk")[0];
    let plan = lowered.plans[id.index()].as_ref().expect("a loop");
    assert_eq!(plan.homes, vec![Home::Loop, Home::Parameter]);
    assert_eq!(self_calls(&lowered.units[id.index()]), 0);
    assert!(unit_violations(&program, &lowered, id).is_empty());
}

#[test]
fn unpublished_self_calls_stay_calls() {
    for source in [
        // A current-frame borrow and a post-call contract keep ordinary frames.
        "fn descend(n:u64,text:String)->u64 { let next = std@1/u64.to_string(n); if n == 0u64 { string.byte_len(text) } else { descend(n-1u64,next) } }\nfn main()->u64 {descend(1000u64,std@1/u64.to_string(1u64))}",
        "fn descend(n:u64)->u64 ensures result == 0u64 { if n == 0u64 {0u64} else {descend(n-1u64)} }\nfn main()->u64 {descend(1000u64)}",
        // Pending resource finalization.
        "resource type R {n:u64} drop(x) { let _ = std@1/u64.to_string(x.n) }\nfn descend(n:u64)->u64 { let r = R(n:n); if n == 0u64 { r.n } else { descend(n-1u64) } }\nfn main()->u64 {descend(5u64)}",
        // Not in tail position.
        "fn depth(n:u64)->u64 { if n == 0u64 { 0u64 } else { 1u64 + depth(n-1u64) } }\nfn main()->u64 {depth(5u64)}",
    ] {
        let (program, lowered) = lower(source);
        let id = instances_of(&lowered, "descend")
            .into_iter()
            .chain(instances_of(&lowered, "depth"))
            .next()
            .unwrap();
        assert!(lowered.plans[id.index()].is_none(), "{source}");
        let unit = &lowered.units[id.index()];
        assert_eq!(self_calls(unit), 1, "{source}\n{}", dump(unit));
        assert!(!has_cycle(unit), "{source}");
        let v = unit_violations(&program, &lowered, id);
        assert!(v.is_empty(), "{source}\n{v:#?}");
    }
}

#[test]
fn earlier_non_tail_recursion_keeps_its_frame_beside_the_loop() {
    // The tail site loops; the earlier recursive call is still a call, so the
    // function does not run in constant stack.
    let source = "fn twice(n:u64,acc:u64)->u64 { if n == 0u64 { acc } else { let inner = twice(n-1u64, 0u64); twice(n-1u64, acc+inner) } }\nfn main()->u64 {twice(3u64,0u64)}";
    let (program, lowered) = lower(source);
    let id = instances_of(&lowered, "twice")[0];
    let unit = &lowered.units[id.index()];
    assert!(
        lowered.plans[id.index()].is_some(),
        "{:?}",
        lowered.declined
    );
    assert!(has_cycle(unit));
    assert_eq!(self_calls(unit), 1, "{}", dump(unit));
    assert!(unit_violations(&program, &lowered, id).is_empty());
}

/// Rewrite one definition's entry body in a compiled program.
fn forge(program: &mut Program, name: &str, f: impl FnOnce(&mut hirsel_tir::Body)) {
    let path = program
        .definitions
        .keys()
        .find(|p| p.0.name == name)
        .unwrap()
        .clone();
    let mut def = (*program.definitions[&path]).clone();
    let entry = def.entry.0 as usize;
    f(&mut def.bodies[entry]);
    program.definitions.insert(path, std::sync::Arc::new(def));
}

/// A ConstructorHole witness on a plain self tail names no constructor chain:
/// normalization refuses it (LANG-10).
#[test]
fn a_forged_constructor_hole_is_rejected_and_its_twin_loops() {
    let program = compile(LOOPS[0].0);
    // Accepted twin: the published SelfTail site lowers.
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    let id = instances_of(&lowered, "sum")[0];
    assert!(lowered.plans[id.index()].is_some());
    let mut forged = program.clone();
    forge(&mut forged, "sum", |body| {
        body.tail_sites[0].kind = TailKind::ConstructorHole(vec![0]);
    });
    match hirsel_rc::import::lower(&forged, &target()) {
        Err(LowerError::Normalize(_, NormalizeError::ConstructorHole { .. })) => {}
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("a ConstructorHole site without its chain lowered"),
    }
}

#[test]
fn a_forged_site_on_a_non_tail_call_is_declined_and_stays_a_call() {
    let source = "fn depth(n:u64)->u64 { if n == 0u64 { 0u64 } else { 1u64 + depth(n-1u64) } }\nfn main()->u64 {depth(5u64)}";
    let mut program = compile(source);
    forge(&mut program, "depth", |body| {
        let call_block = body
            .blocks
            .iter()
            .position(|b| matches!(b.terminator, hirsel_tir::Terminator::Call { .. }))
            .unwrap();
        body.tail_sites.push(hirsel_tir::TailSite {
            owner_body: hirsel_tir::BodyId(0),
            call_block: hirsel_tir::BlockId(call_block as u32),
            kind: TailKind::SelfTail,
            owned_loop_arguments: Vec::new(),
        });
    });
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    let id = instances_of(&lowered, "depth")[0];
    assert!(lowered.plans[id.index()].is_none());
    assert_eq!(lowered.declined.len(), 1, "{:?}", lowered.declined);
    assert_eq!(self_calls(&lowered.units[id.index()]), 1);
    assert!(unit_violations(&program, &lowered, id).is_empty());
}

fn permutations(n: u32) -> Vec<Vec<u32>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for p in permutations(n - 1) {
        for i in 0..=p.len() {
            let mut q = p.clone();
            q.insert(i, n - 1);
            out.push(q);
        }
    }
    out
}

const HELPER: &str = "fn mix(a:u64, b:bool, c:u8)->u64 { if b && c == 2u8 { a + 1u64 } else { a } }\nfn walk(n:u64, a:u64)->u64 { if n == 0u64 { a } else { walk(n-1u64, mix(a, true, 2u8)) } }\nfn main()->u64 { mix(1u64, false, 3u8) + walk(3u64, 0u64) }";

#[test]
fn every_helper_parameter_permutation_updates_all_call_sites_and_the_abi() {
    for order in permutations(3) {
        let (program, mut lowered) = lower(HELPER);
        let id = instances_of(&lowered, "mix")[0];
        let before: Vec<_> = lowered.table.fn_abis[lowered.instances.abis[id.index()].0 as usize]
            .parameters
            .clone();
        let abi = permute_parameters(&mut lowered, id, &order).unwrap();
        let after = &lowered.table.fn_abis[abi.0 as usize].parameters;
        for (i, &old) in order.iter().enumerate() {
            assert_eq!(after[i].layout, before[old as usize].layout);
            assert_eq!(after[i].convention, before[old as usize].convention);
            assert_eq!(after[i].source, i as u32);
        }
        for p in &lowered.instances.view(id).parameters {
            assert_eq!(order[p.instance as usize], p.source);
        }
        let v = violations(&program, &lowered);
        assert!(v.is_empty(), "{order:?}: {v:#?}");
    }
}

#[test]
fn a_permutation_missing_one_call_site_is_rejected() {
    let (program, mut lowered) = lower(HELPER);
    let id = instances_of(&lowered, "mix")[0];
    // The permuted twin is accepted.
    let mut twin = lower(HELPER).1;
    permute_parameters(&mut twin, id, &[2, 0, 1]).unwrap();
    assert!(violations(&program, &twin).is_empty());
    // Undo the transform in `main` only: its call no longer matches the ABI.
    permute_parameters(&mut lowered, id, &[2, 0, 1]).unwrap();
    let main = instances_of(&lowered, "main")[0];
    let unit = &mut lowered.units[main.index()];
    for a in &mut unit.pools.call_arguments {
        a.parameter = [1, 2, 0][a.parameter as usize % 3];
    }
    assert!(!unit_violations(&program, &lowered, main).is_empty());
}

#[test]
fn closures_and_function_values_keep_their_erased_parameter_order() {
    let source = "fn mix(a:u64, b:bool)->u64 { if b { a } else { 0u64 } }\nfn main()->u64 { let f = mix; f(1u64, true) }";
    let (_, mut lowered) = lower(source);
    let id = instances_of(&lowered, "mix")[0];
    assert_eq!(
        permute_parameters(&mut lowered, id, &[1, 0]),
        Err(PermuteError::Escapes)
    );
    assert_eq!(
        permute_parameters(&mut lowered, id, &[0, 0]),
        Err(PermuteError::NotAPermutation)
    );
}
