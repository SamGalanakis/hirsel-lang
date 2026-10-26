//! The source catch-all survives import; exhaustive matches retain their trap.
use crate::rc_support as support;
use hirsel_lir::ir::Terminator;

#[test]
fn only_a_source_catch_all_receives_the_default_tag_edge() {
    let (_, lowered) = support::lower_valid(
        "type Ev = Move(u64) | Pair(u64,u64) | Tick
fn wildcard(e: Ev) -> u64 { match e { Pair(_,_) => 1, _ => 0 } }
fn exhaustive(e: Ev) -> u64 { match e { Move(_) => 0, Pair(_,_) => 1, Tick => 0 } }
fn nested_test(e: Ev) -> u64 { match e { Pair(0,_) => 1, Move(_) => 2, _ => 0 } }
pub fn main() -> Result[(),u64] { Error(wildcard(Tick) + exhaustive(Tick) + nested_test(Tick)) }",
    );
    for (name, catch_all) in [
        ("wildcard", true),
        ("exhaustive", false),
        ("nested_test", false),
    ] {
        let ids = support::instances_of(&lowered, name);
        assert_eq!(ids.len(), 1);
        let unit = &lowered.units[ids[0].index()];
        let mut switches = 0;
        for term in &unit.terminators {
            if let Terminator::Switch {
                arms, otherwise, ..
            } = term
            {
                switches += 1;
                let default = unit.tables.edges[otherwise.index()].target;
                let common = arms
                    .get(&unit.pools.switch_arms)
                    .unwrap()
                    .iter()
                    .any(|a| unit.tables.edges[a.edge.index()].target == default);
                assert_eq!(common, catch_all, "{name}\n{}", support::dump(unit));
                if !catch_all {
                    assert!(matches!(
                        unit.terminators[default.index()],
                        Terminator::Unreachable
                    ));
                }
            }
        }
        assert!(switches > 0, "{name}");
    }
}

#[test]
fn a_borrowed_soa_list_count_reaches_the_vectorizer_before_unrolling() {
    use hirsel_driver::compiler_adapters;
    use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
    use hirsel_engine::{
        QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
    };
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "tag-count.hir",
                "@soa
 type Ev = Move(u64) | Pair(u64,u64) | Tick
 fn arm_count(xs: List[Ev], initial: u64) -> u64 {
   let mut count = initial
   for e in xs { match e { Pair(_,_) => { count += 1 }, _ => {} } }
   count
 }
 pub fn main(args: Args) -> Result[(),u64] {
   let mut xs: List[Ev] = []
   for i in 0u64..arg.len(args) { list.push(mut xs, if i % 3 == 1 { Pair(i,i) } else { Tick }) }
   Error(arm_count(xs, arg.len(args)))
 }
"
                .into(),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let ir = emit_ir(&ready, OptLevel::Default).unwrap();
        assert!(ir.contains("count.headroom"), "{ir}");
        assert!(ir.contains("llvm.vector.reduce.add"), "{ir}");
        assert!(ir.contains("count.checked"), "{ir}");
    });
}
