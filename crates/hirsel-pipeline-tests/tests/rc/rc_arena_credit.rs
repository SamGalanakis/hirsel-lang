//! General result-placement summaries and arena child-credit elision, through
//! source import, every C boundary and H's physical holder audit.
use crate::rc_support;
use hirsel_lir::{ir::*, layout_types::TargetSpec};

#[test]
fn sealed_regions_33_native_scope_grant_closes_without_dropping_borrowed_storage() {
    check("fn main() { arena.with(fn(a: Arena) {}); }", &[], &[]);
}

#[test]
fn sealed_regions_33_native_grant_cleanup_refuses_unqualified_source_claims() {
    use hirsel_lir::validate::ViolationKind;
    use std::sync::Arc;

    // The accepted control is the exact source above, already exercised through
    // C/Ready/H. Mutate after portable import to test the downstream producer
    // boundary, rather than counting a source/V rejection as a C adversary.
    let original = rc_support::compile("fn main() { arena.with(fn(a: Arena) {}); }");
    let path = rc_support::path("main");
    let body = original.definitions[&path]
        .bodies
        .iter()
        .position(|body| body.arena_scope_grant.is_some())
        .expect("qualified callback control");
    for mutation in 0..5 {
        let mut program = original.clone();
        let definition = Arc::make_mut(program.definitions.get_mut(&path).unwrap());
        let body = &mut definition.bodies[body];
        match mutation {
            0 => body.arena_scope_grant = None,
            1 => {
                body.arena_scope_grant.as_mut().unwrap().arena_parameter = hirsel_types::ParamId(1)
            }
            2 => {
                body.arena_scope_grant.as_mut().unwrap().callback_parameter =
                    hirsel_types::ParamId(1)
            }
            3 => {
                body.arena_scope_grant.as_mut().unwrap().intrinsic = rc_support::path("arena.with")
            }
            4 => {
                let root = body.parameters[0];
                let place = body
                    .cleanup_nodes
                    .iter()
                    .find_map(|node| match node.action {
                        hirsel_tir::CleanupAction::Drop { place, .. }
                            if body.places[place.0 as usize].root == root =>
                        {
                            Some(place)
                        }
                        _ => None,
                    })
                    .expect("original formal grant cleanup");
                body.places[place.0 as usize].projection =
                    vec![hirsel_tir::Projection::Field(0)].into();
            }
            _ => unreachable!(),
        }
        let lowered = hirsel_rc::import::lower(&program, &rc_support::target());
        if mutation == 4 {
            // Arena has no field zero. A projected cleanup must be refused by
            // typed import, before it can erase a whole formal's grant cleanup.
            assert!(matches!(
                lowered,
                Err(hirsel_rc::import::LowerError::Import(_, _))
            ));
        } else {
            let lowered = lowered.expect("unqualified metadata does not change the storage type");
            assert!(
                rc_support::violations(&program, &lowered)
                    .iter()
                    .any(|(_, rows)| {
                        rows.iter()
                            .any(|row| matches!(row.kind, ViolationKind::BorrowedStorage(_)))
                    }),
                "mutation {mutation} erased borrowed storage cleanup"
            );
        }
    }
}

fn check(source: &str, expected: &[(&str, Option<u32>)], elided: &[(&str, bool)]) {
    let program = rc_support::compile(source);
    for target in TargetSpec::executed() {
        let built = hirsel_driver::native::build_program(
            &program,
            rc_support::manifest(&program),
            &hirsel_driver::native::Options::new(target),
        )
        .unwrap();
        for &(name, claim) in expected {
            let unit = built
                .units
                .iter()
                .map(|u| u.unit())
                .find(|u| u.header.definition.0.name == name)
                .unwrap();
            assert_eq!(
                built.table.fn_abis[unit.header.abi.0 as usize].result_arena, claim,
                "{name}"
            );
        }
        for &(name, claim) in elided {
            let unit = built
                .units
                .iter()
                .map(|u| u.unit())
                .find(|u| u.header.definition.0.name == name)
                .unwrap();
            let sites: Vec<_> = unit
                .instructions
                .iter()
                .filter_map(|inst| {
                    let Inst::Construct {
                        result, construct, ..
                    } = *inst
                    else {
                        return None;
                    };
                    let site = &unit.tables.constructs[construct.index()];
                    (unit.types[unit.values[result.index()].ty.index()].repr == ValueRepr::Block
                        && matches!(
                            site.placement,
                            Some(Placement::Fixed(StorageOwner::Arena(_)))
                        ))
                    .then_some(site)
                })
                .collect();
            assert!(!sites.is_empty(), "{name}");
            assert_eq!(sites.iter().all(|s| s.arena_no_drop), claim, "{name}");
        }

        let run = hirsel_heapcheck::run(
            &hirsel_heapcheck::Program {
                invocation_bodies: built.operations.invocation_bodies.clone(),
                invocations: built.operations.invocations.clone(),
                pretransfer_adapters: built.operations.pretransfer_adapters.clone(),
                target: built.target.clone(),
                layouts: built.table.clone(),
                units: built.raw_units(),
                statics: built.statics.clone(),
            },
            &rc_support::path("main"),
            &[],
            &mut hirsel_heapcheck::NoOperations,
            &hirsel_heapcheck::Config::default(),
        );
        assert!(run.result.is_ok(), "{:?}", run.result);
    }
}

#[test]
fn result_arena_summaries_cover_ordinary_and_mutually_recursive_calls() {
    check(
        r#"
    type Tree = Leaf | Branch(left: box Tree, right: box Tree)
    fn odd(a: Arena, n: u64) -> (box Tree) in a {
      if n == 0u64 { box Leaf() }
      else { box Branch(left: even(a,n-1u64),right: even(a,n-1u64)) }
    }
    fn even(a: Arena, n: u64) -> (box Tree) in a { odd(a,n) }
    fn wrapper(a: Arena) -> (box Tree) in a { even(a,3u64) }
    fn main() { arena.with(fn(a: Arena) { let tree = wrapper(a); }); }
    "#,
        &[("odd", Some(0)), ("even", Some(0)), ("wrapper", Some(0))],
        &[("odd", true)],
    );
}

#[test]
fn heap_return_paths_remove_claims_from_the_entire_recursive_group() {
    check(
        r#"
    type Item { n: u64 }
    fn first(a: Arena, n: u64) -> (box Item) in a {
      if n == 0u64 { let h: (box Item) in heap = box Item(n:6u64); h }
      else { second(a,n-1u64) }
    }
    fn second(a: Arena, n: u64) -> (box Item) in a { first(a,n) }
    fn wrapper(a: Arena) -> (box Item) in a { second(a,2u64) }
    fn main() { arena.with(fn(a: Arena) { let item = wrapper(a); }); }
    "#,
        &[("first", None), ("second", None), ("wrapper", None)],
        &[],
    );
}

#[test]
fn a_heap_child_stored_into_an_arena_keeps_its_dynamic_check() {
    check(
        r#"
    type Item { n: u64 }
    type Parent { child: box Item }
    fn make(a: Arena, take child: box Item) -> (box Parent) in a { box Parent(child:take child) }
    fn main() {
      arena.with(fn(a: Arena) {
        let child: (box Item) in heap = box Item(n:6u64)
        let parent = make(a,take child);
      });
    }
    "#,
        &[("make", Some(0))],
        &[("make", false)],
    );
}

#[test]
fn a_summary_selects_the_actual_arena_parameter() {
    check(
        r#"
    type Item { n: u64 }
    fn make(a: Arena, b: Arena) -> (box Item) in b { box Item(n:6u64) }
    fn main() {
      arena.with(fn(a: Arena) {
        arena.with(fn(b: Arena) { let item = make(a,b); });
      });
    }
    "#,
        &[("make", Some(1))],
        &[("make", true)],
    );
}
