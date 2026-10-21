//! Lowering of real checked source, from hirsel-lower's unit tests.
use crate::check_support::*;
use hirsel_tir::*;
use hirsel_types::UseKind as U;

#[test]
fn copies_of_flat_indexed_elements_remain_copies_across_iterations() {
    for source in [
        "flat type Item {text:String,n:u64}\nflat type Items {items:List[Item]}\nfn first(xs:Items)->u64 {let mut n:u64=0\nfor x in xs.items {n=x.n}\nn}",
        "flat type Items {items:List[u64]}\nfn first(xs:Items)->u64 {let mut n:u64=0\nfor x in xs.items {n=x}\nn}",
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine
            .sources
            .add("flat-index-copy.hir", source.into())
            .unwrap();
        let diagnostics =
            hirsel_engine::check_snapshot(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}

#[test]
fn a_callee_snapshot_survives_its_slots_replacement_in_an_argument() {
    let source = "fn make(base:u64)->fn(u64)->u64 uses {} {let payload:List[u64]=[base]; fn(x:u64)->u64 uses {} {payload[0u64]+x}}\nfn snapshot()->u64 {let mut f=make(7u64);let other=make(99u64);f({f=other;0u64})}\nfn main()->u64 {snapshot()}";
    assert_eq!(bits(execute_source(source)), 7);
}

#[test]
fn scoped_captures_retain_external_origins_without_duplicate_owner_slots() {
    for source in [
        "fn probe(s:String,x:String of s)->u64 {let f=fn(){string.byte_len(x)};f()}",
        "fn probe(a:Arena)->u64 {let x:String in a=\"x\";let f=fn(){string.byte_len(x)};f()}",
        "fn probe()->once fn()->u64 {let x:String in heap=\"x\";fn(){string.byte_len(take x)}}",
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine.sources.add("capture.hir", source.into()).unwrap();
        let diagnostics =
            hirsel_engine::check_snapshot(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(diagnostics.is_empty(), "{source}: {diagnostics:?}");
    }
}

/// Spec issue #50: a block, branch or arm is transparent to a borrowing
/// consumer. No path may leave an owned value or a slot of its own behind.
#[test]
fn borrowed_tails_and_joins_lend_storage_that_outlives_them() {
    use hirsel_tir::{CleanupAction, InstructionKind, Rvalue, Storage};
    use hirsel_types::UseKind;
    // A borrowed tail naming outer storage is that place: the loop body can
    // run again because no non-owning slot was started for it.
    let run = execute_source(
        "fn one(s: String) -> u64 { 1 }\nfn main() -> u64 { let s: String = \"ab\"\n let mut n: u64 = 0\n while n < 3 { n = n + one({ s }) }\n n }",
    );
    assert_eq!(bits(run), 3);
    // One arm borrows and two compute: the computed strings stay in their own
    // temporaries, are lent to the join, and are dropped where they were made.
    let run = execute_source(
        "fn one(s: String) -> u64 { 1 }\nfn pick(s: String, n: u64) -> u64 { one(match n { 0 => s, 1 => \"four\", _ => \"x\" }) }\nfn main() -> u64 { let s: String = \"ab\"\n pick(s, 0) * 100 + pick(s, 1) * 10 + pick(s, 2) }",
    );
    assert_eq!(bits(run), 111);
    // A plain part read out of a block is a copy: mutating it leaves the
    // storage unchanged.
    let run = execute_source(
        "fn one(mut a: u64) { a = 9 }\nfn main() -> u64 { let mut xs: Array[u64] = [1, 2]\n one(mut { xs }[1])\n xs[0] * 10 + xs[1] }",
    );
    assert_eq!(bits(run), 12);
    let (types, d) = source_definition(
        "fn one(s: String) -> u64 { 1 }\nfn main(s: String, n: u64) -> u64 { one(if n == 0 { s } else { \"other\" }) }",
    );
    let body = &d.bodies[d.entry.0 as usize];
    let literal = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| match i.kind {
            InstructionKind::Assign { destination, value }
                if matches!(body.rvalues[value.0 as usize], Rvalue::Constant(_))
                    && matches!(
                        types
                            .get(body.places[destination.0 as usize].ty)
                            .unwrap()
                            .kind,
                        hirsel_types::TypeKind::Primitive(hirsel_types::Primitive::String)
                    ) =>
            {
                Some(body.places[destination.0 as usize].root)
            }
            _ => None,
        })
        .unwrap();
    // The literal's value ends in an owned temporary that is lent...
    let lent: Vec<_> = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter_map(|i| match i.kind {
            InstructionKind::BeginLoan { place, .. }
                if body.locals[body.places[place.0 as usize].root.0 as usize].storage
                    == Storage::Temporary =>
            {
                Some(body.places[place.0 as usize].root)
            }
            _ => None,
        })
        .collect();
    assert_eq!(lent.len(), 1, "{lent:?}");
    assert_ne!(lent[0], literal);
    // ...is never moved into the join, and is dropped only if that path ran.
    assert!(body.rvalues.iter().all(|r| !matches!(r,
        Rvalue::Use(o) if o.usage == UseKind::Take
            && body.places[o.place.0 as usize].root == lent[0])));
    assert!(body.cleanup_nodes.iter().any(|node| matches!(node.action,
        CleanupAction::Drop { place, if_live: true }
            if body.places[place.0 as usize].root == lent[0])));
}

#[test]
fn scalar_and_nominal_plain_values_have_no_cleanup_actions() {
    for source in [
        "fn main()->u8 { let a=3u8\nlet b=a\nb }",
        "type Pair { a:u8, b:u8 }\nfn main()->u8 { let p=Pair(a:3u8,b:4u8)\np.a }",
        "fn f[T:plain](a:T)->T { let b=a\nb }\nfn main()->u8 { f(3u8) }",
    ] {
        let (_types, d) = source_definition(source);
        assert!(d.bodies.iter().all(|body| body.cleanup_nodes.is_empty()));
        assert_eq!(bits(execute_source(source)), 3);
    }
}

#[test]
fn generic_plain_bounds_omit_cleanup_without_losing_lexical_evidence() {
    let (_types, d) = source_definition("fn main[T:plain](a:T)->T { let b=a\nb }");
    assert!(d.bodies.iter().all(|body| body.cleanup_nodes.is_empty()));
}

fn indirect_callee_storage(source: &str) -> Vec<Storage> {
    let (_types, definition) = source_definition(source);
    definition
        .bodies
        .iter()
        .flat_map(|body| {
            body.blocks.iter().filter_map(|block| {
                let Terminator::Call {
                    callee: Callee::Indirect { value, .. },
                    ..
                } = &block.terminator
                else {
                    return None;
                };
                assert_eq!(value.usage, U::Borrow);
                Some(body.locals[body.places[value.place.0 as usize].root.0 as usize].storage)
            })
        })
        .collect()
}

#[test]
fn stable_heap_environment_calls_borrow_the_original_slot() {
    let source = "fn make(base:u64)->fn(u64)->u64 uses {} {let payload:List[u64]=[base]; fn(x:u64)->u64 uses {} {payload[0u64]+x}}\nfn main()->u64 {let mut f=make(7u64);f(0u64)}";
    assert_eq!(indirect_callee_storage(source), vec![Storage::Local]);
    assert_eq!(bits(execute_source(source)), 7);
}

#[test]
fn stable_indexed_calls_allow_readonly_sibling_arguments() {
    let source = "type Stage {call:fn(u64,u64)->u64 uses {},environment:u64}\nfn add(x:u64,n:u64)->u64 {x+n}\nfn main()->u64 {let stages:List[Stage]=[Stage(call:add,environment:7u64)];stages[0u64].call(0u64,stages[0u64].environment)}";
    assert_eq!(indirect_callee_storage(source), vec![Storage::Local]);
    assert_eq!(bits(execute_source(source)), 7);
}

#[test]
fn stable_calls_keep_borrowed_result_dependencies_through_last_use() {
    let source = "fn main()->u64 {let bytes:Bytes=b\"abc\";let f=fn(n:u64)->Bytes in env uses {} {bytes};let view=f(0u64);let _=view;3u64}";
    assert_eq!(indirect_callee_storage(source), vec![Storage::Local]);
    assert_eq!(bits(execute_source(source)), 3);
}

#[test]
fn replacing_an_indexed_callee_invokes_the_previously_selected_environment() {
    let source = "type Stage {call:fn(u64)->u64 uses {}}\nfn make(base:u64)->fn(u64)->u64 uses {} {let payload:List[u64]=[base];fn(n:u64)->u64 uses {} {payload[0u64]+n}}\nfn main()->u64{let mut stages:List[Stage]=[Stage(call:make(7u64))];stages[0u64].call({stages=[Stage(call:make(99u64))];0u64})}";
    assert_eq!(bits(execute_source(source)), 7);
}

#[test]
fn an_owned_related_yield_can_return_without_copying_it() {
    // MODE-1, OWN-1, SEM-4, and core library ordinary
    // borrowed loop sources: the owned yield retains s, not cursor access.
    for source in [
        include_str!("../../../../conformance/SEM/SEM-4-view-return-source-run.hir"),
        include_str!("../../../../conformance/SEM/SEM-4-view-return-transparent-run.hir"),
        include_str!("../../../../conformance/SEM/SEM-4-view-return-direct-run.hir"),
    ] {
        for source in [
            source.to_owned(),
            source
                .replace("return piece", "return copy piece")
                .replace("=> piece,", "=> copy piece,"),
        ] {
            let source = source
                .replace("pub fn main(rt:", "pub fn entry(rt:")
                .replace("fn first(", "fn main(")
                .replace("first(", "main(");
            source_definition(&source);
        }
    }
}

#[test]
fn propagation_reads_borrowed_result_payloads_through_their_carrier() {
    let (types, d) = source_definition(
        "fn part(s: String) -> Result[String, String] in s { Ok(s) }\nfn main(s: String) -> Result[u64, String] in s { let _ = part(s)?\nOk(1) }",
    );
    let body = &d.bodies[d.entry.0 as usize];
    let relation = |ty: hirsel_types::TypeId| types.get(ty).unwrap().modes.relation.clone();
    // OWN-8: both payload places of the `in s` carrier keep its bound.
    let payloads: Vec<_> = (0..body.places.len())
        .filter(|&i| {
            let projection = &body.places[i].projection;
            projection.len() == 1
                && matches!(
                    projection.first(),
                    Some(Projection::VariantField { field: 0, .. })
                )
        })
        .collect();
    assert_eq!(payloads.len(), 2, "{payloads:?}");
    for &payload in &payloads {
        let place = &body.places[payload];
        let carrier = relation(body.locals[place.root.0 as usize].ty);
        assert!(!carrier.is_global());
        assert_eq!(relation(place.ty), carrier, "{place:?}");
    }
    // As `return Error(e)`: the propagated Result is built at its error's
    // locality, and returning it checks that against `in s`.
    let built = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| match i.kind {
            InstructionKind::Assign { destination, value } => match &body.rvalues[value.0 as usize]
            {
                Rvalue::Construct { fields, .. }
                    if fields.len() == 1
                        && payloads.contains(&(fields[0].value.place.0 as usize)) =>
                {
                    Some((destination, fields[0].value.place))
                }
                _ => None,
            },
            _ => None,
        });
    let (destination, error) = built.expect("propagated Error construction");
    assert_eq!(
        relation(body.places[destination.0 as usize].ty),
        relation(body.places[error.0 as usize].ty)
    );
    assert_ne!(body.places[destination.0 as usize].ty, body.result);
}
